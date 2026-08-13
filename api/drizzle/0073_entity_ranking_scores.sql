-- Explore feed "Best" ranking — Phase A storage and scoring primitives.
--
-- Implements the global best score from the "Explore feed ranking: Best and For
-- You" PRD: a time-invariant additive log-time score combining quality, an
-- entity-type weight, and recency.
--
--   qualityScore  = Wilson lower bound on (positive, negative) with a pseudo-count prior
--   rankingScore  = log(max(qualityScore, floor)) + log(w_type) + intrinsic + created_at/tau
--
-- Deliberate design choices, all driven by measurements taken 2026-08-13:
--
-- * Side table, not columns on `entities`. That table is 48.9M rows / 18 GB with
--   four indexes; the repo already established the side-table pattern with
--   `global_scores` / `local_scores` + a SETOF function + a PostGraphile plugin.
--
-- * DDL and functions only — NO backfill here. `api`'s initContainer runs
--   `db:migrate`, so a 48M-row UPDATE in a migration would stall every deploy.
--   The backfill ships as a separate, batched, resumable script.
--
-- * The score is time-invariant: `created_at / tau` means an entity's score moves
--   only when its votes, type weight, or structure change. No periodic decay
--   sweep, and cursor pagination stays stable between vote events.
--
-- * `intrinsic_score` is NOT an optional extra. Measured: of 48.8M entities only
--   2,650 have any votes (median 1, max 36) and just 56 have 5 or more. With
--   almost every entity sitting at the identical prior, log(quality) is constant
--   and the ranking would collapse to pure recency. The intrinsic term is the only
--   quality gradient that exists between zero-vote entities, so it carries Phase A.
--   It is capped so that it stays subordinate to real votes once votes exist.

-- ---------------------------------------------------------------------------
-- Tunables. A table rather than constants so tau, the prior, and the caps can be
-- retuned without a deploy — the PRD calls all of them out as needing
-- experimentation.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS "entity_ranking_config" (
  "id" boolean PRIMARY KEY DEFAULT true,
  -- Recency divisor in seconds. 45000 is Reddit's hot-score constant and gives a
  -- ~12.5h "one unit of score" cadence. NOTE: it must be applied to SECONDS.
  -- scoring-service's dead calculate_hot_score applied it to hours, making the
  -- time term ~3600x too weak; do not repeat that.
  "tau_seconds" numeric NOT NULL DEFAULT 45000,
  -- Wilson z. 1.96 = 95% confidence lower bound.
  "wilson_z" numeric NOT NULL DEFAULT 1.96,
  -- Pseudo-count prior. Kept deliberately weak (PRD: "keep k at the low end so the
  -- first few real votes visibly move rank") because early votes are the scarcest
  -- signal on the platform.
  "prior_positive" numeric NOT NULL DEFAULT 1,
  "prior_negative" numeric NOT NULL DEFAULT 1,
  -- Floor under qualityScore before log(), so a fully-downvoted entity yields a
  -- finite score instead of -Infinity.
  "quality_floor" numeric NOT NULL DEFAULT 0.01,
  -- Ceiling on the intrinsic term, in score units. Must stay well below the spread
  -- that real votes produce so votes dominate once they exist.
  "intrinsic_cap" numeric NOT NULL DEFAULT 0.5,
  -- Structural counts at which the intrinsic term saturates.
  "intrinsic_property_target" integer NOT NULL DEFAULT 10,
  "intrinsic_relation_target" integer NOT NULL DEFAULT 5,
  "updated_at" timestamp with time zone NOT NULL DEFAULT now(),
  CONSTRAINT "entity_ranking_config_single_row" CHECK ("id"),
  CONSTRAINT "entity_ranking_config_tau_positive" CHECK ("tau_seconds" > 0),
  CONSTRAINT "entity_ranking_config_floor_positive" CHECK ("quality_floor" > 0),
  CONSTRAINT "entity_ranking_config_targets_positive"
    CHECK ("intrinsic_property_target" > 0 AND "intrinsic_relation_target" > 0)
);
--> statement-breakpoint
INSERT INTO "entity_ranking_config" ("id") VALUES (true) ON CONFLICT ("id") DO NOTHING;
--> statement-breakpoint

-- ---------------------------------------------------------------------------
-- Type-level weighting. A News story is a better feed unit than a Text block.
-- Weight > 1 boosts, < 1 demotes, absent defaults to 1.0. Applied as log(weight)
-- so the additive log-time score stays time-invariant.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS "entity_type_weights" (
  "type_id" uuid PRIMARY KEY,
  "weight" numeric NOT NULL DEFAULT 1.0,
  "note" text,
  "updated_at" timestamp with time zone NOT NULL DEFAULT now(),
  -- A weight can demote but never effectively exclude: exclusion is an editorial
  -- decision and must not be reachable by the weighting math (PRD floor guardrail).
  CONSTRAINT "entity_type_weights_range" CHECK ("weight" > 0.1 AND "weight" <= 10.0)
);
--> statement-breakpoint

-- Excluded types are enforced at candidate generation, NOT as a zero weight —
-- excluded entities must never consume candidate slots.
CREATE TABLE IF NOT EXISTS "entity_type_exclusions" (
  "type_id" uuid PRIMARY KEY,
  "note" text,
  "updated_at" timestamp with time zone NOT NULL DEFAULT now()
);
--> statement-breakpoint

-- ---------------------------------------------------------------------------
-- The scores themselves.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS "entity_ranking_scores" (
  "entity_id" uuid PRIMARY KEY,
  "quality_score" numeric NOT NULL,
  "intrinsic_score" numeric NOT NULL DEFAULT 0,
  "ranking_score" numeric NOT NULL,
  -- Inputs retained for debugging and for the PRD's "expose them on the node"
  -- requirement, so a surprising ranking can be explained without recomputation.
  "positive" bigint NOT NULL DEFAULT 0,
  "negative" bigint NOT NULL DEFAULT 0,
  "type_weight" numeric NOT NULL DEFAULT 1.0,
  "updated_at" timestamp with time zone NOT NULL DEFAULT now()
);
--> statement-breakpoint
-- The Phase A sort key. `entity_id` breaks ties so cursor pagination is total.
CREATE INDEX IF NOT EXISTS "entity_ranking_scores_ranking_desc_idx"
  ON "entity_ranking_scores" ("ranking_score" DESC, "entity_id" DESC);
--> statement-breakpoint
-- Supports the PRD's numeric threshold filter on rankingScore.
CREATE INDEX IF NOT EXISTS "entity_ranking_scores_quality_idx"
  ON "entity_ranking_scores" ("quality_score" DESC);
--> statement-breakpoint

-- ---------------------------------------------------------------------------
-- Wilson lower confidence bound on a Bernoulli proportion.
--
-- Chosen over a raw net score because net score treats 100-up/90-down the same as
-- 10-up/0-down. Wilson penalises uncertainty, so a low-vote newcomer cannot
-- outrank a well-established entity on a lucky first vote — a Phase A acceptance
-- criterion.
--
-- IMMUTABLE: pure arithmetic, so Postgres can inline it and use it in indexes.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.wilson_lower_bound(
  positive numeric,
  negative numeric,
  z numeric DEFAULT 1.96,
  prior_positive numeric DEFAULT 1,
  prior_negative numeric DEFAULT 1
) RETURNS numeric
LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $$
DECLARE
  p_obs   numeric := GREATEST(COALESCE(positive, 0), 0) + GREATEST(COALESCE(prior_positive, 0), 0);
  n_obs   numeric := GREATEST(COALESCE(negative, 0), 0) + GREATEST(COALESCE(prior_negative, 0), 0);
  total   numeric := p_obs + n_obs;
  phat    double precision;
  zf      double precision := COALESCE(z, 1.96)::double precision;
  nf      double precision;
  lower   double precision;
BEGIN
  -- No observations and no prior: undefined proportion, so report the floor rather
  -- than dividing by zero.
  IF total <= 0 THEN
    RETURN 0;
  END IF;

  nf := total::double precision;
  phat := (p_obs / total)::double precision;

  lower := (phat + (zf * zf) / (2 * nf)
            - zf * sqrt((phat * (1 - phat) + (zf * zf) / (4 * nf)) / nf))
           / (1 + (zf * zf) / nf);

  -- Clamp: floating-point error can push the bound marginally outside [0,1].
  RETURN LEAST(GREATEST(lower, 0), 1)::numeric;
END;
$$;
--> statement-breakpoint

-- ---------------------------------------------------------------------------
-- Structural quality for entities with no votes: property completeness and
-- relation count, each saturating, summed and capped.
--
-- This is the sparse-state substitute for a vote signal. It is intentionally a
-- small additive bonus, not a multiplier, so it can order zero-vote entities among
-- themselves without ever overturning a real vote signal.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.entity_intrinsic_score(
  property_count integer,
  relation_count integer,
  cap numeric DEFAULT 0.5,
  property_target integer DEFAULT 10,
  relation_target integer DEFAULT 5
) RETURNS numeric
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
  SELECT LEAST(
    COALESCE(cap, 0.5),
    COALESCE(cap, 0.5) * (
        0.6 * LEAST(GREATEST(COALESCE(property_count, 0), 0)::numeric
                    / GREATEST(COALESCE(property_target, 10), 1), 1.0)
      + 0.4 * LEAST(GREATEST(COALESCE(relation_count, 0), 0)::numeric
                    / GREATEST(COALESCE(relation_target, 5), 1), 1.0)
    )
  );
$$;
--> statement-breakpoint

-- ---------------------------------------------------------------------------
-- The Phase A ranking score.
--
-- `created_at` is text-encoded epoch seconds in this schema, so the caller passes
-- it already cast. The cast is write-time only — the score is stored, never
-- computed per query.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.entity_ranking_score(
  quality_score numeric,
  type_weight numeric,
  intrinsic_score numeric,
  created_at_epoch bigint,
  tau_seconds numeric DEFAULT 45000,
  quality_floor numeric DEFAULT 0.01
) RETURNS numeric
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
  SELECT ln(GREATEST(COALESCE(quality_score, 0), GREATEST(COALESCE(quality_floor, 0.01), 1e-9)))
       + ln(GREATEST(COALESCE(type_weight, 1.0), 1e-9))
       + COALESCE(intrinsic_score, 0)
       + COALESCE(created_at_epoch, 0)::numeric / GREATEST(COALESCE(tau_seconds, 45000), 1);
$$;
--> statement-breakpoint

-- ---------------------------------------------------------------------------
-- Incremental recompute for a set of entities. Called by the writer when votes
-- change; also the unit of work for the batched backfill.
--
-- Because the score is time-invariant this is the ONLY thing that ever needs to
-- run — there is no scheduled decay sweep.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.refresh_entity_ranking_scores(entity_ids uuid[])
RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
  cfg          entity_ranking_config;
  affected     integer;
  types_prop   constant uuid := '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1';
BEGIN
  SELECT * INTO cfg FROM entity_ranking_config WHERE id;

  WITH target AS (
    SELECT e.id, NULLIF(e.created_at, '')::bigint AS created_epoch
    FROM entities e
    WHERE e.id = ANY(entity_ids)
  ),
  votes AS (
    -- vote_kind = 0 is curation; stance and veracity are separate axes and must not
    -- feed the feed's quality term.
    SELECT vc.object_id AS id,
           SUM(vc.positive)::bigint AS positive,
           SUM(vc.negative)::bigint AS negative
    FROM votes_count vc
    WHERE vc.object_type = 0 AND vc.vote_kind = 0
      AND vc.object_id = ANY(entity_ids)
    GROUP BY vc.object_id
  ),
  props AS (
    SELECT v.entity_id AS id, count(DISTINCT v.property_id)::integer AS property_count
    FROM values v WHERE v.entity_id = ANY(entity_ids) GROUP BY v.entity_id
  ),
  rels AS (
    -- System relations are plumbing, not curatorial substance, so they must not
    -- inflate the structural signal.
    SELECT r.from_entity_id AS id, count(*)::integer AS relation_count
    FROM relations r
    WHERE r.from_entity_id = ANY(entity_ids) AND r.is_system = false
    GROUP BY r.from_entity_id
  ),
  weights AS (
    -- An entity may declare several types; take the strongest weight so an
    -- explicit boost is never diluted by a co-assigned generic type.
    SELECT r.from_entity_id AS id, MAX(w.weight) AS type_weight
    FROM relations r
    JOIN entity_type_weights w ON w.type_id = r.to_entity_id
    WHERE r.from_entity_id = ANY(entity_ids) AND r.type_id = types_prop
    GROUP BY r.from_entity_id
  ),
  computed AS (
    SELECT t.id,
           public.wilson_lower_bound(COALESCE(v.positive, 0), COALESCE(v.negative, 0),
                                     cfg.wilson_z, cfg.prior_positive, cfg.prior_negative) AS quality_score,
           public.entity_intrinsic_score(COALESCE(p.property_count, 0), COALESCE(rl.relation_count, 0),
                                          cfg.intrinsic_cap, cfg.intrinsic_property_target,
                                          cfg.intrinsic_relation_target) AS intrinsic_score,
           COALESCE(w.type_weight, 1.0) AS type_weight,
           COALESCE(v.positive, 0) AS positive,
           COALESCE(v.negative, 0) AS negative,
           t.created_epoch
    FROM target t
    LEFT JOIN votes v   ON v.id = t.id
    LEFT JOIN props p   ON p.id = t.id
    LEFT JOIN rels rl   ON rl.id = t.id
    LEFT JOIN weights w ON w.id = t.id
  )
  INSERT INTO entity_ranking_scores AS s
    (entity_id, quality_score, intrinsic_score, ranking_score, positive, negative, type_weight, updated_at)
  SELECT c.id, c.quality_score, c.intrinsic_score,
         public.entity_ranking_score(c.quality_score, c.type_weight, c.intrinsic_score,
                                      c.created_epoch, cfg.tau_seconds, cfg.quality_floor),
         c.positive, c.negative, c.type_weight, now()
  FROM computed c
  ON CONFLICT (entity_id) DO UPDATE SET
    quality_score   = EXCLUDED.quality_score,
    intrinsic_score = EXCLUDED.intrinsic_score,
    ranking_score   = EXCLUDED.ranking_score,
    positive        = EXCLUDED.positive,
    negative        = EXCLUDED.negative,
    type_weight     = EXCLUDED.type_weight,
    updated_at      = EXCLUDED.updated_at;

  GET DIAGNOSTICS affected = ROW_COUNT;
  RETURN affected;
END;
$$;
