-- GEO-3461 phase 1: Best scores debates on how much of them people watch.
--
-- WHY. GEO-3454 replaces a debate's up/down with Agree/Disagree on its claim, so the curation
-- votes that fed a debate's quality and participation terms stop arriving. What does arrive is
-- playback: personalization.feed_item_signals_hourly (0109), synced hourly from analytics by
-- ranking-indexer's feed_signals_sync. In order of importance, per the ticket:
--
--   1. watch time per impression on Explore  — how much of the debate a shown card earns;
--   2. completed plays (covered at least 90% of the media);
--   3. plays.
--
-- Claim votes cast from the debate player are phase 1b (they need ClickHouse's debate_id on the
-- vote event); discounting muted autoplay and idle tabs is phase 2. Neither is here.
--
-- THE TERM, for a Debate-typed entity, over the last `watch_window_days`:
--
--   ratio      = (explore_watch_seconds + prior × reference) / (impressions + prior)
--   watch_score = clamp(±cap,  ratio_weight     × ln(ratio / reference)
--                            + completed_weight × ln(1 + completed_plays)
--                            + plays_weight     × ln(1 + plays))
--
-- The ratio is smoothed toward `reference` seconds per impression with `prior` pseudo-impressions,
-- so a debate with no signals scores exactly 0, and a debate shown 7 times cannot outrank one
-- shown 200 times on a lucky ratio alone. The log puts it in the same units as every other term:
-- one unit is tau (100000 s, about 28 hours) of recency, so the cap of 8 is worth about 9 days.
-- Below the reference a debate loses score: a card people scroll past is a signal too.
--
-- Measured on testnet 2026-10-23 over 30 days: 367 debates with signals; median 5 impressions,
-- 9 plays, 161 s watched, 0 completed; p99 254 impressions, 327 plays, 6 completed. Explore
-- averages 9.7 s of watch per impression, hence the 10 s reference. With these weights the
-- simulated top 66 of Explore keeps today's claim/debate split (13/53); ratio 4 drops claims to
-- 11, ratio 6 to 3. Debates with high watch per impression rise (01a15f62, 57.7 s/impression:
-- rank 33 → 10), those people scroll past fall (01a15a31, 2.2 s: 5 → 31).
--
-- Every weight 0 turns the term off: watch_score is 0 and ranking_score is 0107's exactly.
--
-- FRESHNESS. The term lives in refresh_entity_ranking_scores, so any re-score (votes, comments,
-- the new-entity sweep) picks it up. Watching changes nothing those see, so
-- refresh_debate_watch_scores() re-scores the debates with signals, and feed_signals_sync calls
-- it in the same transaction that replaces the signals: a debate's score moves when its signals
-- do, and a dry run rolls both back.

ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "watch_window_days" integer NOT NULL DEFAULT 30;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "watch_prior_impressions" numeric NOT NULL DEFAULT 20;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "watch_reference_seconds" numeric NOT NULL DEFAULT 10;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "watch_ratio_weight" numeric NOT NULL DEFAULT 3.0;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "watch_completed_weight" numeric NOT NULL DEFAULT 1.0;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "watch_plays_weight" numeric NOT NULL DEFAULT 0.25;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "watch_cap" numeric NOT NULL DEFAULT 8;
--> statement-breakpoint
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'entity_ranking_config_watch_terms_valid') THEN
    ALTER TABLE "entity_ranking_config" ADD CONSTRAINT "entity_ranking_config_watch_terms_valid"
      CHECK ("watch_window_days" > 0 AND "watch_prior_impressions" > 0
             AND "watch_reference_seconds" > 0 AND "watch_ratio_weight" >= 0
             AND "watch_completed_weight" >= 0 AND "watch_plays_weight" >= 0 AND "watch_cap" >= 0);
  END IF;
END
$$;
--> statement-breakpoint

-- Stored for explainability, like the other terms, and already included in ranking_score. 0 for
-- everything that is not a debate. A constant default, so adding it rewrites no rows.
ALTER TABLE "entity_ranking_scores"
  ADD COLUMN IF NOT EXISTS "watch_score" numeric NOT NULL DEFAULT 0;
--> statement-breakpoint

-- The watch term for those of `entity_ids` that are debates, with its inputs. One row per debate,
-- including debates with no signals (all zero); nothing for anything else.
CREATE OR REPLACE FUNCTION public.debate_watch_scores(entity_ids uuid[])
RETURNS TABLE (entity_id uuid, impressions bigint, plays bigint, explore_watch_seconds double precision,
               completed_plays bigint, watch_score numeric)
LANGUAGE sql STABLE AS $$
  WITH cfg AS (
    SELECT c.* FROM public.entity_ranking_config c WHERE c.id
  ),
  debates AS (
    SELECT DISTINCT ty.from_entity_id AS id
    FROM public.relations ty
    WHERE ty.from_entity_id = ANY(entity_ids)
      AND ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid      -- Types
      AND ty.to_entity_id = 'fd51f935-2063-4617-be39-7b672b23364c'::uuid -- Debate
  ),
  signals AS (
    SELECT f.item_id AS id,
           sum(f.impressions)::bigint AS impressions,
           sum(f.plays)::bigint AS plays,
           sum(f.explore_watch_ms)::double precision / 1000 AS explore_watch_seconds,
           sum(f.completed_plays)::bigint AS completed_plays
    FROM personalization.feed_item_signals_hourly f CROSS JOIN cfg
    WHERE f.item_id IN (SELECT id FROM debates)
      AND f.hour >= now() - make_interval(days => cfg.watch_window_days)
    GROUP BY f.item_id
  ),
  inputs AS (
    SELECT d.id,
           COALESCE(s.impressions, 0) AS impressions,
           COALESCE(s.plays, 0) AS plays,
           COALESCE(s.explore_watch_seconds, 0) AS explore_watch_seconds,
           COALESCE(s.completed_plays, 0) AS completed_plays
    FROM debates d LEFT JOIN signals s ON s.id = d.id
  )
  SELECT i.id, i.impressions, i.plays, i.explore_watch_seconds, i.completed_plays,
         GREATEST(-cfg.watch_cap, LEAST(cfg.watch_cap,
             cfg.watch_ratio_weight
               * ln(((GREATEST(i.explore_watch_seconds, 0)::numeric
                      + cfg.watch_prior_impressions * cfg.watch_reference_seconds)
                     / (GREATEST(i.impressions, 0) + cfg.watch_prior_impressions))
                    / cfg.watch_reference_seconds)
           + cfg.watch_completed_weight * ln(1 + GREATEST(i.completed_plays, 0)::numeric)
           + cfg.watch_plays_weight * ln(1 + GREATEST(i.plays, 0)::numeric)))
  FROM inputs i CROSS JOIN cfg
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.debate_watch_scores(uuid[]) IS E'@omit';
--> statement-breakpoint

-- The score choke point as 0107 left it, plus the watch term. Everything else is unchanged.
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
    -- DISTINCT space_id: the space a reply was authored from is who wrote it. Direct replies only.
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
  -- GEO-3150: rows only for topic debates; everything else gets 0 below.
  topic AS (
    SELECT tds.entity_id AS id, tds.topic_score
    FROM public.topic_debate_scores(entity_ids) tds
  ),
  -- GEO-3461: rows only for debates; everything else gets 0 below.
  watch AS (
    SELECT dws.entity_id AS id, dws.watch_score
    FROM public.debate_watch_scores(entity_ids) dws
  ),
  computed AS (
    SELECT t.id,
           public.wilson_lower_bound(COALESCE(v.positive, 0), COALESCE(v.negative, 0),
                                     cfg.wilson_z, cfg.prior_positive, cfg.prior_negative) AS quality_score,
           public.entity_intrinsic_score(COALESCE(p.property_count, 0), COALESCE(rl.relation_count, 0),
                                          cfg.intrinsic_cap, cfg.intrinsic_property_target,
                                          cfg.intrinsic_relation_target) AS intrinsic_score,
           public.entity_participation_score(COALESCE(s.positive, 0) + COALESCE(s.negative, 0)
                                              + COALESCE(v.positive, 0) + COALESCE(v.negative, 0),
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
           tp.topic_score,
           COALESCE(wa.watch_score, 0) AS watch_score,
           t.created_epoch
    FROM target t
    LEFT JOIN votes v    ON v.id = t.id
    LEFT JOIN stance s   ON s.id = t.id
    LEFT JOIN comments cm ON cm.id = t.id
    LEFT JOIN props p    ON p.id = t.id
    LEFT JOIN rels rl    ON rl.id = t.id
    LEFT JOIN weights w  ON w.id = t.id
    LEFT JOIN topic tp   ON tp.id = t.id
    LEFT JOIN watch wa   ON wa.id = t.id
  )
  INSERT INTO entity_ranking_scores AS s
    (entity_id, quality_score, intrinsic_score, participation_score, comment_score, ranking_score,
     positive, negative, stance_positive, stance_negative, commenter_count, type_weight, topic_score,
     watch_score, updated_at)
  SELECT c.id, c.quality_score, c.intrinsic_score, c.participation_score, c.comment_score,
         -- Not a topic debate: exactly 0098's expression, plus the watch term (0 if not a debate).
         CASE WHEN c.topic_score IS NULL
              THEN public.entity_ranking_score(c.quality_score, c.type_weight, c.intrinsic_score,
                                                c.participation_score, c.comment_score, c.created_epoch,
                                                cfg.tau_seconds, cfg.quality_floor)
              ELSE public.entity_ranking_score(c.quality_score, c.type_weight, c.intrinsic_score,
                                                c.participation_score, c.comment_score, c.created_epoch,
                                                cfg.tau_seconds, cfg.quality_floor)
                   + c.topic_score
         END + c.watch_score,
         c.positive, c.negative, c.stance_positive, c.stance_negative, c.commenter_count,
         c.type_weight, COALESCE(c.topic_score, 0), c.watch_score, now()
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
    topic_score         = EXCLUDED.topic_score,
    watch_score         = EXCLUDED.watch_score,
    updated_at          = EXCLUDED.updated_at;

  GET DIAGNOSTICS affected = ROW_COUNT;

  -- ---- entity_type_ranking reconcile, for these ids only (unchanged from 0098) ----
  DELETE FROM entity_type_ranking etr
  WHERE etr.entity_id = ANY(entity_ids)
    AND NOT EXISTS (
      SELECT 1 FROM relations r
      WHERE r.from_entity_id = etr.entity_id
        AND r.type_id = types_prop
        AND r.to_entity_id = etr.type_id
    );

  INSERT INTO entity_type_ranking (type_id, entity_id, ranking_score)
  SELECT DISTINCT r.to_entity_id, r.from_entity_id, ers.ranking_score
  FROM relations r
  JOIN entity_ranking_scores ers ON ers.entity_id = r.from_entity_id
  WHERE r.from_entity_id = ANY(entity_ids)
    AND r.type_id = types_prop
  ON CONFLICT (type_id, entity_id) DO UPDATE SET
    ranking_score = EXCLUDED.ranking_score;

  -- ---- entity_topic_ranking reconcile, for these ids only (unchanged from 0098) ----
  DELETE FROM entity_topic_ranking etr
  WHERE etr.entity_id = ANY(entity_ids)
    AND (
      NOT EXISTS (
        SELECT 1 FROM relations r
        WHERE r.from_entity_id = etr.entity_id AND r.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid AND r.to_entity_id = etr.topic_id
      )
      OR etr.type_id NOT IN (SELECT public.entity_topic_ranking_kinds(etr.entity_id))
      OR NOT public.entity_topic_ranking_eligible(etr.entity_id)
    );

  INSERT INTO entity_topic_ranking (topic_id, type_id, entity_id, ranking_score, created_epoch)
  SELECT DISTINCT tp.to_entity_id, k.kind, tp.from_entity_id, ers.ranking_score, CASE WHEN ce.created_at ~ '^[0-9]+$' THEN ce.created_at::bigint END
  FROM relations tp
  CROSS JOIN LATERAL public.entity_topic_ranking_kinds(tp.from_entity_id) AS k(kind)
  JOIN entity_ranking_scores ers ON ers.entity_id = tp.from_entity_id
  JOIN entities ce ON ce.id = tp.from_entity_id
  WHERE tp.from_entity_id = ANY(entity_ids)
    AND tp.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid
    AND public.entity_topic_ranking_eligible(tp.from_entity_id)
  ON CONFLICT (topic_id, type_id, entity_id) DO UPDATE SET
    ranking_score = EXCLUDED.ranking_score,
    created_epoch = EXCLUDED.created_epoch;

  -- The SCORE row count, as callers and the backfill script read it.
  RETURN affected;
END;
$$;
--> statement-breakpoint

-- Re-scores every debate whose watch term can have moved: those with signals in the window, and
-- those still carrying a nonzero term (their signals aged out, or the weights changed). Called by
-- ranking-indexer's feed_signals_sync after each replace, in its transaction. Returns the debates
-- scored.
CREATE OR REPLACE FUNCTION public.refresh_debate_watch_scores()
RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
  ids uuid[];
  window_days integer;
BEGIN
  SELECT c.watch_window_days INTO window_days FROM public.entity_ranking_config c WHERE c.id;
  SELECT coalesce(array_agg(DISTINCT ty.from_entity_id), '{}') INTO ids
  FROM public.relations ty
  WHERE ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid
    AND ty.to_entity_id = 'fd51f935-2063-4617-be39-7b672b23364c'::uuid
    AND (EXISTS (SELECT 1 FROM personalization.feed_item_signals_hourly f
                 WHERE f.item_id = ty.from_entity_id
                   AND f.hour >= now() - make_interval(days => window_days))
         OR EXISTS (SELECT 1 FROM public.entity_ranking_scores s
                    WHERE s.entity_id = ty.from_entity_id AND s.watch_score <> 0));
  IF cardinality(ids) = 0 THEN
    RETURN 0;
  END IF;
  RETURN public.refresh_entity_ranking_scores(ids);
END;
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.refresh_debate_watch_scores() IS E'@omit';
--> statement-breakpoint

-- Apply the term to the debates that already have signals.
SELECT public.refresh_debate_watch_scores();
