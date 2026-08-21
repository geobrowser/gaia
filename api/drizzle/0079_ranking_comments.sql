-- Comments feed the ranking score.
--
-- WHY, AND WHAT THE NUMBERS SAY
--
-- Preston asked whether replies to an entity should count as engagement. They should,
-- and the distribution says how much. Measured against the live graph (chain 55516):
--
--            entities with >=1   >=2   >=5   >=10   max
--   curation votes      2541     638    59     15    36
--   stance positions     235      94    17      4    12
--   comments             243      89    18      7    52
--
-- Comments and positions are almost the same shape — 243 vs 235 entities touched, 18 vs
-- 17 above five — so a comment term wants the SAME scale as participation, not a token
-- one. The earlier worry was the opposite (that comments would be plentiful enough to
-- swamp the score); they are not, they are 3.5x sparser than curation votes.
--
-- Comments do have the longest tail: one entity has 52. That is what the cap is for, and
-- why the input is DISTINCT COMMENTERS rather than raw comments — a comment costs nothing
-- to make and nothing stops one account leaving forty. `commenter_space_id` is how the
-- notification indexer already attributes a reply (`models.rs`, `edit_space_id`), so this
-- uses the same definition rather than inventing one.
--
-- SHAPE
--
-- Reuses `entity_participation_score`: it is already `LEAST(cap, weight * ln(1 + n))`,
-- which is the shape this wants, and a second copy would be a second thing to keep in
-- step. Separate weight and cap columns so comments can be tuned — or zeroed — without
-- touching positions. Both signals sit in the same additive slot as engagement.
--
-- SHIPS INERT. `comment_weight` defaults to 0, so applying this changes no score. Arming
-- it is a config write, the same way 0078 was armed.
--
-- ⚠️ ONE THING THIS MIGRATION CANNOT DO
--
-- `refresh_entity_ranking_scores` runs only when a vote is written — see
-- `vote-indexer/src/storage.rs`, whose own comment warns that adding a term without
-- extending its trigger "looks exactly like the feature not working". A comment is not a
-- vote, so nothing here causes a recompute when one arrives: an entity picks up its
-- comment score the next time it is re-scored for some other reason.
--
-- That is the same staleness `intrinsic_score` already lives with (a new relation does not
-- trigger a recompute either), so this is consistent rather than novel. But for an
-- engagement signal it matters more, and it is why arming `comment_weight` should wait on
-- a recompute path. Options, none of which belong in a migration: a trigger on `relations`
-- for reply-to inserts (puts scoring in the KG write path), enqueueing a recompute from
-- whichever indexer writes the relation, or a periodic sweep of recently-commented
-- entities. Deliberately left as a decision rather than assumed.

ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "comment_weight" numeric NOT NULL DEFAULT 0;
--> statement-breakpoint
-- 30 matches the armed participation cap: at tau 100000 that is ~34.7 days of recency
-- equivalent, which is the most any single engagement signal should be able to buy.
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "comment_cap" numeric NOT NULL DEFAULT 30;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  DROP CONSTRAINT IF EXISTS "entity_ranking_config_comment_nonneg";
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD CONSTRAINT "entity_ranking_config_comment_nonneg"
  CHECK ("comment_weight" >= 0 AND "comment_cap" >= 0);
--> statement-breakpoint

-- The computed term and its input, stored for the same reason 0078 stored the stance
-- counts: an ordering nobody expected has to be explainable from the API alone.
ALTER TABLE "entity_ranking_scores"
  ADD COLUMN IF NOT EXISTS "comment_score" numeric NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE "entity_ranking_scores"
  ADD COLUMN IF NOT EXISTS "commenter_count" bigint NOT NULL DEFAULT 0;
--> statement-breakpoint

-- ---------------------------------------------------------------------------
-- The ranking score, now with the comment term.
--
-- DROP first. Postgres binds to the exact-arity definition, so leaving the 7-argument
-- version in place means `refresh_entity_ranking_scores` keeps calling it — silently, with
-- the comment term dropped on the floor. Nothing errors; the feature just does nothing.
-- Same trap 0078 documented when it went from 6 arguments to 7.
-- ---------------------------------------------------------------------------
DROP FUNCTION IF EXISTS public.entity_ranking_score(
  numeric, numeric, numeric, numeric, bigint, numeric, numeric
);
--> statement-breakpoint
CREATE OR REPLACE FUNCTION public.entity_ranking_score(
  quality_score numeric,
  type_weight numeric,
  intrinsic_score numeric,
  participation_score numeric,
  comment_score numeric,
  created_at_epoch bigint,
  tau_seconds numeric DEFAULT 45000,
  quality_floor numeric DEFAULT 0.01
) RETURNS numeric
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
  SELECT ln(GREATEST(COALESCE(quality_score, 0), GREATEST(COALESCE(quality_floor, 0.01), 1e-9)))
       + ln(GREATEST(COALESCE(type_weight, 1.0), 1e-9))
       + COALESCE(intrinsic_score, 0)
       + COALESCE(participation_score, 0)
       + COALESCE(comment_score, 0)
       + COALESCE(created_at_epoch, 0)::numeric / GREATEST(COALESCE(tau_seconds, 45000), 1);
$$;
--> statement-breakpoint

-- ---------------------------------------------------------------------------
-- Recompute, now counting distinct commenters.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.refresh_entity_ranking_scores(entity_ids uuid[])
RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
  cfg          entity_ranking_config;
  affected     integer;
  types_prop   constant uuid := '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1';
  reply_to     constant uuid := '310d4a24-0e5b-451c-b215-1bfce40d0fe6';
BEGIN
  SELECT * INTO cfg FROM entity_ranking_config WHERE id;

  WITH target AS (
    SELECT e.id, NULLIF(e.created_at, '')::bigint AS created_epoch
    FROM entities e
    WHERE e.id = ANY(entity_ids)
  ),
  votes AS (
    SELECT vc.object_id AS id,
           SUM(vc.positive)::bigint AS positive,
           SUM(vc.negative)::bigint AS negative
    FROM votes_count vc
    WHERE vc.object_type = 0 AND vc.vote_kind = 0
      AND vc.object_id = ANY(entity_ids)
    GROUP BY vc.object_id
  ),
  stance AS (
    SELECT vc.object_id AS id,
           SUM(vc.positive)::bigint AS positive,
           SUM(vc.negative)::bigint AS negative
    FROM votes_count vc
    WHERE vc.object_type = 0 AND vc.vote_kind = 1
      AND vc.object_id = ANY(entity_ids)
    GROUP BY vc.object_id
  ),
  comments AS (
    -- DISTINCT space_id, not count(*): the space a reply was authored from is who wrote
    -- it (`notification-indexer` resolves `commenter_space_id` from the edit's space the
    -- same way), and a comment is free to make, so raw volume is the one input here an
    -- individual can run up on their own.
    --
    -- Direct replies to the entity only. A reply to a *comment* points at that comment,
    -- so thread depth does not inflate the parent — engagement with the entity is what
    -- this is measuring.
    SELECT r.to_entity_id AS id,
           count(DISTINCT r.space_id)::bigint AS commenter_count
    FROM relations r
    WHERE r.type_id = reply_to AND r.to_entity_id = ANY(entity_ids)
    GROUP BY r.to_entity_id
  ),
  props AS (
    SELECT v.entity_id AS id, count(DISTINCT v.property_id)::integer AS property_count
    FROM values v WHERE v.entity_id = ANY(entity_ids) GROUP BY v.entity_id
  ),
  rels AS (
    SELECT r.from_entity_id AS id, count(*)::integer AS relation_count
    FROM relations r
    WHERE r.from_entity_id = ANY(entity_ids) AND r.is_system = false
    GROUP BY r.from_entity_id
  ),
  weights AS (
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
           public.entity_participation_score(COALESCE(cm.commenter_count, 0),
                                              cfg.comment_weight, cfg.comment_cap)
             AS comment_score,
           COALESCE(w.type_weight, 1.0) AS type_weight,
           COALESCE(v.positive, 0) AS positive,
           COALESCE(v.negative, 0) AS negative,
           COALESCE(s.positive, 0) AS stance_positive,
           COALESCE(s.negative, 0) AS stance_negative,
           COALESCE(cm.commenter_count, 0) AS commenter_count,
           t.created_epoch
    FROM target t
    LEFT JOIN votes v    ON v.id = t.id
    LEFT JOIN stance s   ON s.id = t.id
    LEFT JOIN comments cm ON cm.id = t.id
    LEFT JOIN props p    ON p.id = t.id
    LEFT JOIN rels rl    ON rl.id = t.id
    LEFT JOIN weights w  ON w.id = t.id
  )
  INSERT INTO entity_ranking_scores AS s
    (entity_id, quality_score, intrinsic_score, participation_score, comment_score, ranking_score,
     positive, negative, stance_positive, stance_negative, commenter_count, type_weight, updated_at)
  SELECT c.id, c.quality_score, c.intrinsic_score, c.participation_score, c.comment_score,
         public.entity_ranking_score(c.quality_score, c.type_weight, c.intrinsic_score,
                                      c.participation_score, c.comment_score, c.created_epoch,
                                      cfg.tau_seconds, cfg.quality_floor),
         c.positive, c.negative, c.stance_positive, c.stance_negative, c.commenter_count,
         c.type_weight, now()
  FROM computed c
  ON CONFLICT (entity_id) DO UPDATE SET
    quality_score       = EXCLUDED.quality_score,
    intrinsic_score     = EXCLUDED.intrinsic_score,
    participation_score = EXCLUDED.participation_score,
    comment_score       = EXCLUDED.comment_score,
    ranking_score       = EXCLUDED.ranking_score,
    positive            = EXCLUDED.positive,
    negative            = EXCLUDED.negative,
    stance_positive     = EXCLUDED.stance_positive,
    stance_negative     = EXCLUDED.stance_negative,
    commenter_count     = EXCLUDED.commenter_count,
    type_weight         = EXCLUDED.type_weight,
    updated_at          = EXCLUDED.updated_at;

  GET DIAGNOSTICS affected = ROW_COUNT;
  RETURN affected;
END;
$$;
--> statement-breakpoint

-- Visible on the node for the same reason 0078 exposed positions: the first thing anyone
-- does with a surprising rank is query its inputs.
CREATE OR REPLACE FUNCTION public.entities_commenters(e public.entities)
RETURNS bigint LANGUAGE sql STABLE AS $$
  SELECT rs.commenter_count FROM public.entity_ranking_scores rs WHERE rs.entity_id = e.id;
$$;
--> statement-breakpoint
CREATE OR REPLACE FUNCTION public.entities_comment_score(e public.entities)
RETURNS numeric LANGUAGE sql STABLE AS $$
  SELECT rs.comment_score FROM public.entity_ranking_scores rs WHERE rs.entity_id = e.id;
$$;
