-- Explore feed "Best" — let stance votes (positions) raise a claim's rank.
--
-- Why
-- ---
-- Reported 2026-08-19: on the explore feed sorted by Best, claims carrying
-- positions rank below claims with none. Two independent causes, both here:
--
-- 1. Positions never entered the score. `refresh_entity_ranking_scores` reads
--    `votes_count WHERE vote_kind = 0` (curation) only, so the AGREED/DISAGREED
--    triple contributed nothing. 0073's comment cited "PRD §8 Q4" for that —
--    the PRD has no §8 and no numbered questions. It was written 2026-07-09,
--    four weeks BEFORE `vote_kind` existed (0071, 2026-08-05), so it only ever
--    said "up/down votes". Curation-only was an implementation choice inherited
--    from that wording, not a decision anyone made about stance. Claims cannot
--    be upvoted in the UI at all, so the one signal they do carry was the one
--    signal the ranking ignored.
--
-- 2. A bounded quality term cannot compete with unbounded recency. `ln(wilson)`
--    spans only [-4.6, 0] and `intrinsic` adds at most 0.5, so the entire
--    quality range is ~5.1 score units. At the live tau that is ~5.9 days:
--    anything created a week later outranks anything older however good it is.
--    The reported pair is 13.55 days apart, and the older claim would still lose
--    with a PERFECT curation score.
--
-- So a capped bonus term is the wrong shape. Adding `intrinsic`-sized
-- participation (cap 0.5 ≈ 14 hours) would have changed nothing observable and
-- looked like the feature did not work.
--
-- Shape
-- -----
--   participation = LEAST(cap, weight * ln(1 + agrees + disagrees))
--
-- Logarithmic and effectively unbounded, which is what the PRD's own reference
-- (Reddit hot score) actually does: `log10(score) + seconds/45000`. Reddit's
-- score term is an unbounded log precisely so popularity can outrun age. Phase A
-- made every quality input bounded and kept the unbounded time term, which is
-- the whole reason recency swamps the score. This restores the asymmetry the
-- formula family assumes.
--
-- DIRECTION-AGNOSTIC on purpose: `agrees + disagrees`, not `agrees - disagrees`.
-- Feeding stance direction into a quality term ranks uncontested claims above
-- contested ones, which is backwards for a debate product — a claim split 50/50
-- is the most interesting thing in the feed, not the least. What is being
-- rewarded here is that people engaged, not that they agreed. Veracity
-- (`vote_kind = 2`) stays out entirely; it is an assertion about truth, a third
-- axis, and mixing it in would conflate "discussed" with "verified".
--
-- Sizing, against the reported pair (13.55 days apart = 12.11 units to overcome).
-- NOTE these are at the LIVE tau of ~100000 (one unit ~= 27.8h), not the 45000
-- default in this table — the v2 database was retuned out of band. The required
-- weight scales with tau, so re-derive it if tau ever moves; the 0078 test suite
-- sets tau explicitly for exactly this reason.
--   weight 2  ->  5 positions = 3.6 units  (~4.1 days)   — reorders within a week
--   weight 7  ->  5 positions = 12.5 units (~14.5 days)  — flips the reported pair
--   weight 7  ->  1 position  = 4.9 units  (~5.6 days)
-- Defensible for this product: only 2,650 of 48.8M entities carry any vote, so
-- one position is a genuinely rare signal. But it is a product call, which is
-- why the default is 0.
--
-- SHIPS INERT. `participation_weight` defaults to 0, so applying this migration
-- changes no score. Arming it is a config UPDATE plus a re-backfill, and both
-- are reversible the same way. That split is deliberate: the score is stored, so
-- any weight change reorders the global feed for every user on the next
-- backfill, and that must be an explicit act rather than a side effect of a
-- deploy.
--
-- `participation_cap` is a manipulation guardrail, not a tuning knob. Positions
-- are cheap to manufacture; the cap bounds what a brigade can buy. 10 units is
-- ~11.6 days at the live tau.

-- ---------------------------------------------------------------------------
-- Config
-- ---------------------------------------------------------------------------
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "participation_weight" numeric NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "participation_cap" numeric NOT NULL DEFAULT 10;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  DROP CONSTRAINT IF EXISTS "entity_ranking_config_participation_nonneg";
--> statement-breakpoint
-- A negative weight would demote engaged claims, which is never intended and is
-- the kind of sign error that is invisible in a stored score.
ALTER TABLE "entity_ranking_config"
  ADD CONSTRAINT "entity_ranking_config_participation_nonneg"
  CHECK ("participation_weight" >= 0 AND "participation_cap" >= 0);
--> statement-breakpoint

-- ---------------------------------------------------------------------------
-- Stored inputs. Retained for the same reason 0073 kept `positive` / `negative`:
-- the PRD requires the ranking be explainable on the node without recomputing.
-- ---------------------------------------------------------------------------
ALTER TABLE "entity_ranking_scores"
  ADD COLUMN IF NOT EXISTS "participation_score" numeric NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE "entity_ranking_scores"
  ADD COLUMN IF NOT EXISTS "stance_positive" bigint NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE "entity_ranking_scores"
  ADD COLUMN IF NOT EXISTS "stance_negative" bigint NOT NULL DEFAULT 0;
--> statement-breakpoint

-- ---------------------------------------------------------------------------
-- Participation term.
--
-- `ln(1 + n)` rather than `ln(n)`: n = 0 must yield 0, not -Infinity, and the
-- first position should be worth the most.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.entity_participation_score(
  positions_total bigint,
  weight numeric DEFAULT 0,
  cap numeric DEFAULT 10
) RETURNS numeric
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
  SELECT LEAST(
    GREATEST(COALESCE(cap, 10), 0),
    GREATEST(COALESCE(weight, 0), 0)
      * ln(1 + GREATEST(COALESCE(positions_total, 0), 0)::numeric)
  );
$$;
--> statement-breakpoint

-- ---------------------------------------------------------------------------
-- The ranking score, now with participation.
--
-- The old 6-argument signature MUST be dropped, not just replaced. Postgres
-- identifies a function by name + argument types and ignores defaults, so
-- `CREATE OR REPLACE` with a 7th defaulted argument creates an OVERLOAD and
-- leaves the 6-arg version in place.
--
-- Verified what that actually does rather than assuming: a 6-argument call does
-- NOT raise "function is not unique" as you might expect. Postgres prefers an
-- exact arity match over one that has to supply a default, so the call binds
-- SILENTLY to the stale 6-arg function — the one whose formula has no
-- participation term. So the hazard is not a loud ambiguity error; it is a
-- surviving second definition of the ranking formula that some future caller
-- resolves to by accident, with no error anywhere. There must be exactly one.
--
-- Safe to drop: the function is hidden from the GraphQL schema
-- (hideProceduresPlugin) and its only caller is refresh_entity_ranking_scores
-- below, which plpgsql resolves at execution time.
-- ---------------------------------------------------------------------------
DROP FUNCTION IF EXISTS public.entity_ranking_score(numeric, numeric, numeric, bigint, numeric, numeric);
--> statement-breakpoint
CREATE OR REPLACE FUNCTION public.entity_ranking_score(
  quality_score numeric,
  type_weight numeric,
  intrinsic_score numeric,
  participation_score numeric,
  created_at_epoch bigint,
  tau_seconds numeric DEFAULT 45000,
  quality_floor numeric DEFAULT 0.01
) RETURNS numeric
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
  SELECT ln(GREATEST(COALESCE(quality_score, 0), GREATEST(COALESCE(quality_floor, 0.01), 1e-9)))
       + ln(GREATEST(COALESCE(type_weight, 1.0), 1e-9))
       + COALESCE(intrinsic_score, 0)
       + COALESCE(participation_score, 0)
       + COALESCE(created_at_epoch, 0)::numeric / GREATEST(COALESCE(tau_seconds, 45000), 1);
$$;
--> statement-breakpoint

-- ---------------------------------------------------------------------------
-- Recompute, now reading stance alongside curation.
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
    -- vote_kind = 0 is curation. Kept as the quality term's only input: a
    -- position is engagement, not an assertion that the entity is good, so it
    -- must not move a Wilson bound on approval.
    SELECT vc.object_id AS id,
           SUM(vc.positive)::bigint AS positive,
           SUM(vc.negative)::bigint AS negative
    FROM votes_count vc
    WHERE vc.object_type = 0 AND vc.vote_kind = 0
      AND vc.object_id = ANY(entity_ids)
    GROUP BY vc.object_id
  ),
  stance AS (
    -- vote_kind = 1 is stance (positions). Summed across spaces to match the
    -- curation CTE. Direction is retained in the stored columns for debugging
    -- but deliberately NOT used in the term — see the header.
    SELECT vc.object_id AS id,
           SUM(vc.positive)::bigint AS positive,
           SUM(vc.negative)::bigint AS negative
    FROM votes_count vc
    WHERE vc.object_type = 0 AND vc.vote_kind = 1
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
           public.entity_participation_score(COALESCE(s.positive, 0) + COALESCE(s.negative, 0),
                                              cfg.participation_weight, cfg.participation_cap)
             AS participation_score,
           COALESCE(w.type_weight, 1.0) AS type_weight,
           COALESCE(v.positive, 0) AS positive,
           COALESCE(v.negative, 0) AS negative,
           COALESCE(s.positive, 0) AS stance_positive,
           COALESCE(s.negative, 0) AS stance_negative,
           t.created_epoch
    FROM target t
    LEFT JOIN votes v   ON v.id = t.id
    LEFT JOIN stance s  ON s.id = t.id
    LEFT JOIN props p   ON p.id = t.id
    LEFT JOIN rels rl   ON rl.id = t.id
    LEFT JOIN weights w ON w.id = t.id
  )
  INSERT INTO entity_ranking_scores AS s
    (entity_id, quality_score, intrinsic_score, participation_score, ranking_score,
     positive, negative, stance_positive, stance_negative, type_weight, updated_at)
  SELECT c.id, c.quality_score, c.intrinsic_score, c.participation_score,
         public.entity_ranking_score(c.quality_score, c.type_weight, c.intrinsic_score,
                                      c.participation_score, c.created_epoch,
                                      cfg.tau_seconds, cfg.quality_floor),
         c.positive, c.negative, c.stance_positive, c.stance_negative, c.type_weight, now()
  FROM computed c
  ON CONFLICT (entity_id) DO UPDATE SET
    quality_score       = EXCLUDED.quality_score,
    intrinsic_score     = EXCLUDED.intrinsic_score,
    participation_score = EXCLUDED.participation_score,
    ranking_score       = EXCLUDED.ranking_score,
    positive            = EXCLUDED.positive,
    negative            = EXCLUDED.negative,
    stance_positive     = EXCLUDED.stance_positive,
    stance_negative     = EXCLUDED.stance_negative,
    type_weight         = EXCLUDED.type_weight,
    updated_at          = EXCLUDED.updated_at;

  GET DIAGNOSTICS affected = ROW_COUNT;
  RETURN affected;
END;
$$;
--> statement-breakpoint

-- ---------------------------------------------------------------------------
-- Expose the new inputs on the node, matching 0074's upvotes / downvotes. The
-- reporter's first move was to query rankingScore + qualityScore + upvotes +
-- downvotes to explain an ordering; positions have to be visible the same way
-- or the next surprising rank is undebuggable from the API.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.entities_agrees(e public.entities)
RETURNS bigint LANGUAGE sql STABLE AS $$
  SELECT rs.stance_positive FROM public.entity_ranking_scores rs WHERE rs.entity_id = e.id;
$$;
--> statement-breakpoint
CREATE OR REPLACE FUNCTION public.entities_disagrees(e public.entities)
RETURNS bigint LANGUAGE sql STABLE AS $$
  SELECT rs.stance_negative FROM public.entity_ranking_scores rs WHERE rs.entity_id = e.id;
$$;
--> statement-breakpoint
CREATE OR REPLACE FUNCTION public.entities_participation_score(e public.entities)
RETURNS numeric LANGUAGE sql STABLE AS $$
  SELECT rs.participation_score FROM public.entity_ranking_scores rs WHERE rs.entity_id = e.id;
$$;
