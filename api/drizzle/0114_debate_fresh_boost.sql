-- New debates get a starting lift in Best until they have been shown enough to be judged.
--
-- WHY. GEO-3454 (geogenesis #2982, 23 Oct 21:50Z) removed a debate's up/down, so debates recorded
-- since then can no longer earn the participation term, while older debates keep theirs. Live on
-- testnet 2026-10-29 the participation term averaged 4.7-5.6 for debates recorded 19-21 Oct and is
-- exactly 0 for every debate recorded since 24 Oct. That is about six days of recency, so new
-- debates rank under week-old ones, and the signals that replaced votes (0111's watch term, 0112's
-- claim votes) need people to see a debate first: debates recorded 27-28 Oct averaged 0.4-2 Explore
-- impressions and no claim votes. Debates under two days old fell from about 60% of viewing on
-- 20-21 Oct to 3-8% on 26-27 Oct.
--
-- THE TERM. One more addend in the watch term, outside its clamp:
--
--   + fresh_boost × max(0, 1 − impressions / fresh_impressions)
--
-- for a debate created within watch_window_days, with impressions counted over that window as the
-- rest of the term counts them. A debate starts `fresh_boost` (5, about what a typical 19-21 Oct
-- debate held in participation) above where it would be, and loses it linearly over its first
-- `fresh_impressions` (50, about what a 19-20 Oct debate collected in its first month). After that
-- only how people watched it counts. It fades with exposure, not time, so a score still never moves
-- merely because an entity ages, except once: at watch_window_days a debate still short of
-- fresh_impressions loses what is left of the boost.
--
-- Outside the clamp so a debate with a bad ratio early on still gets its look, and so the cap keeps
-- bounding what watching alone can do.
--
-- FRESHNESS. refresh_debate_watch_scores re-scored debates with signals or a non-zero watch_score.
-- A debate with no signals yet now has a non-zero watch_score from its first score, so it is kept
-- in that set; it also takes every debate created within the window, so the boost is applied to
-- debates scored before this migration and dropped from debates that age out of the window.
--
-- fresh_boost 0 turns it off: the term is 0112's exactly.

ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "watch_fresh_boost" numeric NOT NULL DEFAULT 5;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "watch_fresh_impressions" numeric NOT NULL DEFAULT 50;
--> statement-breakpoint
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'entity_ranking_config_watch_fresh_valid') THEN
    ALTER TABLE "entity_ranking_config" ADD CONSTRAINT "entity_ranking_config_watch_fresh_valid"
      CHECK ("watch_fresh_boost" >= 0 AND "watch_fresh_impressions" > 0);
  END IF;
END
$$;
--> statement-breakpoint

-- 0112's term plus the fresh boost. Same result type, so no drop.
CREATE OR REPLACE FUNCTION public.debate_watch_scores(entity_ids uuid[])
RETURNS TABLE (entity_id uuid, impressions bigint, plays bigint, explore_watch_seconds double precision,
               completed_plays bigint, claim_votes bigint, watch_score numeric)
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
           sum(f.completed_plays)::bigint AS completed_plays,
           sum(f.debate_claim_votes)::bigint AS claim_votes
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
           COALESCE(s.completed_plays, 0) AS completed_plays,
           COALESCE(s.claim_votes, 0) AS claim_votes,
           CASE WHEN e.created_at ~ '^[0-9]+$' THEN e.created_at::bigint END AS created_epoch
    FROM debates d
    LEFT JOIN signals s ON s.id = d.id
    LEFT JOIN public.entities e ON e.id = d.id
  )
  SELECT i.id, i.impressions, i.plays, i.explore_watch_seconds, i.completed_plays, i.claim_votes,
         GREATEST(-cfg.watch_cap, LEAST(cfg.watch_cap,
             cfg.watch_ratio_weight
               * ln(((GREATEST(i.explore_watch_seconds, 0)::numeric
                      + cfg.watch_prior_impressions * cfg.watch_reference_seconds)
                     / (GREATEST(i.impressions, 0) + cfg.watch_prior_impressions))
                    / cfg.watch_reference_seconds)
           + cfg.watch_completed_weight * ln(1 + GREATEST(i.completed_plays, 0)::numeric)
           + cfg.watch_plays_weight * ln(1 + GREATEST(i.plays, 0)::numeric)
           + cfg.watch_claim_votes_weight * ln(1 + GREATEST(i.claim_votes, 0)::numeric)))
         + CASE WHEN i.created_epoch >= extract(epoch FROM now() - make_interval(days => cfg.watch_window_days))
                THEN cfg.watch_fresh_boost
                     * GREATEST(0, 1 - GREATEST(i.impressions, 0)::numeric / cfg.watch_fresh_impressions)
                ELSE 0 END
  FROM inputs i CROSS JOIN cfg
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.debate_watch_scores(uuid[]) IS E'@omit';
--> statement-breakpoint

-- 0111's selector plus every debate created within the window (see FRESHNESS above).
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
                    WHERE s.entity_id = ty.from_entity_id AND s.watch_score <> 0)
         OR EXISTS (SELECT 1 FROM public.entities e
                    WHERE e.id = ty.from_entity_id AND e.created_at ~ '^[0-9]+$'
                      AND e.created_at::bigint
                          >= extract(epoch FROM now() - make_interval(days => window_days))));
  IF cardinality(ids) = 0 THEN
    RETURN 0;
  END IF;
  RETURN public.refresh_entity_ranking_scores(ids);
END;
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.refresh_debate_watch_scores() IS E'@omit';
--> statement-breakpoint

-- No re-score here, as in 0113: the next hourly feed_signals_sync applies the boost. Bump For you's
-- feed revision (0102), which an ADD COLUMN does not, so its analytics can tell the rankings apart.
UPDATE "entity_ranking_config" SET "updated_at" = now() WHERE "id";
