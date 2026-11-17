-- GEO-3781: agenda claims lift the best-ranked debate on them, and push its siblings down, while the Best debates
-- ranking is on. Ships OFF: both new step settings default to 0, and at 0 the sweep stores no rows and every score
-- is exactly what 0124 gives.
--
-- WHY. Since the GEO-3681 cutover (ranking_places_mode = 2) a ranked debate's trending term is its ranking place
-- value (2600..12500 with one 100-slot source), and an agenda listing adds only its plain boost (8, inside the same
-- GREATEST, so nothing at all for a ranked debate). A debate on today's most timely claim cannot reach the head of
-- Best unless it is ranked there. Boosting every debate on a listed claim equally would fill the head with one claim,
-- so only the best-ranked debate on a claim is lifted, and the other ranked debates on it drop.
--
-- WHAT. Per claim on an agenda dated from 6 days ago to today (every published agenda, block items and Claims
-- relations, as 0113/0118 count claim listings; the newest Date wins when several list it, nothing stacks):
--   1. Its lead is the best ranked debate on it (by weighted place value, then entity_id DESC) that is on no other
--      claim. Ranked means it has a ranking place (so typed Debate, rankable and not pushed down); a debate also typed
--      Claim counts itself as one of its claims, listed or not. Unranked debates get nothing, and a claim with no lead
--      gets nothing.
--   2. The lead is #1 and gets lift = agenda_claim_steps × S × agenda_day_factor^days.
--   3. Every other ranked debate on the claim (one that is on several claims included, whatever its place) is #2..
--      in place order and loses place_drop = agenda_follower_drop_steps × S × agenda_day_factor^days from its place
--      value, never below min(place value, ranking_place_base), so a follower stays above every unranked debate.
--   S = ranking_place_step × the weight of the source that gave the debate its place value (the largest weighted
--   value when several do), so a step is one place in that debate's source (Patrick, 17 Nov). days = New York days
--   since the listing's Date.
--   trending = GREATEST(listing, topic, place value − place_drop) + lift − push-down. There is no ceiling on the lift
--   (Preston, 17 Nov): a debate only jumps while its claim is on a current agenda, which is the point.
--   Debates an agenda names directly (source agenda_debates) and topics are not read: the ranking alone decides the
--   best debate on a claim.
--
-- STORED, NOT INLINE. trending.agenda_boosts is rebuilt by every hourly sweep, after the sweep has written the
-- ranking places, push-downs and agenda listings, and diffed by full row: every debate whose lift or drop came, went
-- or changed is re-scored in the same run (a new day's fade changes every lift, so it re-scores them too). Computing it inline in the terms would hand the lift to two debates
-- between sweeps (a single debate's re-score after a vote cannot see its sibling's move), and would put a window
-- function in the hot path.
--
-- MODE. Rows are kept only while ranking_places_mode = 2 and either step setting is above 0; otherwise the table is
-- emptied and its debates re-scored back. Rolling back to mode 1 or setting both steps to 0 turns it off.
--
-- TWINS NOT PORTED (Patrick, 17 Nov). The Rust shadows of the terms (ranking-indexer agenda_rescore_compute.rs,
-- entity_ranking_score_compute.rs, behind AGENDA_RESCORE_RUST_COMPUTE / ENTITY_RANKING_SCORE_RUST_COMPUTE) and the TS
-- golden twins in api/src/geodb/rankedReads/trending.ts do not model this term. Both Rust shadows stay off; turning
-- either on while agenda_claim_steps or agenda_follower_drop_steps is above 0 would diverge on the boosted debates.
--
-- REVERTS. entity_trending_terms_as and refresh_agenda_listings are re-created here from 0124's bodies; a later
-- migration that re-creates them from 0124's (or 0122's) would silently drop this. The 0126 suite fails if either
-- stops reading trending.agenda_boosts.

ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "agenda_claim_steps" numeric NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "agenda_follower_drop_steps" numeric NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "agenda_day_factor" numeric NOT NULL DEFAULT 0.65;
--> statement-breakpoint
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'entity_ranking_config_agenda_boost_valid') THEN
    ALTER TABLE "entity_ranking_config" ADD CONSTRAINT "entity_ranking_config_agenda_boost_valid"
      CHECK ("agenda_claim_steps" >= 0 AND "agenda_follower_drop_steps" >= 0
             AND "agenda_day_factor" > 0 AND "agenda_day_factor" <= 1);
  END IF;
END
$$;
--> statement-breakpoint

CREATE TABLE IF NOT EXISTS "trending"."agenda_boosts" (
  "entity_id" uuid PRIMARY KEY,                                        -- a ranked debate
  "claim_id" uuid NOT NULL,                                            -- the listed claim it is numbered on
  "rank_on_claim" integer NOT NULL,                                    -- 1 = lifted; 2.. = dropped
  "listed_on" date NOT NULL,                                           -- the claim's newest listing Date
  "lift" numeric NOT NULL,                                             -- added after the GREATEST
  "place_drop" numeric NOT NULL                                        -- taken off the place value, floored
);
--> statement-breakpoint

-- Per placed debate of `entity_ids`, its best weighted place value today and the weight of the source that gave it
-- (ties to the larger weight, then the source name). The value is ranking_place_values' exactly.
CREATE OR REPLACE FUNCTION trending.ranking_place_sources_of(entity_ids uuid[])
RETURNS TABLE (entity_id uuid, place_value numeric, weight numeric)
LANGUAGE sql STABLE AS $$
  WITH cfg AS (SELECT c.*, trending.today() AS today FROM public.entity_ranking_config c WHERE c.id)
  SELECT DISTINCT ON (p.entity_id) p.entity_id,
         s.weight * (cfg.ranking_place_base + cfg.ranking_place_step * (s.slots - p.place + 1))
           * power(s.day_factor, GREATEST(0, cfg.today - trending.epoch_day(e.created_at::bigint))) AS place_value,
         s.weight
  FROM trending.ranking_places p
  JOIN trending.ranking_place_sources s ON s.name = p.source
  JOIN public.entities e ON e.id = p.entity_id AND e.created_at ~ '^[0-9]+$'
  CROSS JOIN cfg
  WHERE p.entity_id = ANY(entity_ids) AND p.place <= s.slots
  ORDER BY p.entity_id, 2 DESC, s.weight DESC, s.name
$$;
--> statement-breakpoint

-- The boosts the stored ranking places, push-downs and claim listings give now (see WHAT). Empty unless the mode is
-- 2 and a step setting is above 0. One row per debate; only rows with a lift or a drop above 0.
CREATE OR REPLACE FUNCTION trending.current_agenda_boosts()
RETURNS TABLE (entity_id uuid, claim_id uuid, rank_on_claim integer, listed_on date, lift numeric,
               place_drop numeric)
LANGUAGE sql STABLE AS $$
  WITH cfg AS (
    SELECT c.*, trending.today() AS today FROM public.entity_ranking_config c
    WHERE c.id AND c.ranking_places_mode = 2 AND (c.agenda_claim_steps > 0 OR c.agenda_follower_drop_steps > 0)
  ),
  -- Listed claims and their newest listing Date in the last 7 days (today included; a future Date waits).
  listed AS (
    SELECT l.entity_id AS claim_id, max(l.listed_on) AS listed_on
    FROM trending.agenda_listings l CROSS JOIN cfg
    WHERE l.source = ANY(trending.claim_sources())
      AND l.listed_on BETWEEN cfg.today - 6 AND cfg.today
    GROUP BY l.entity_id
  ),
  -- Candidate debates: Claims links that count, plus a listed claim that is itself typed Debate.
  placed AS (
    SELECT v.entity_id, v.place_value, v.weight
    FROM trending.ranking_place_sources_of(ARRAY(
      SELECT d.debate_id FROM trending.debate_links(ARRAY(SELECT claim_id FROM listed), '{}') d
      UNION
      SELECT k.entity_id FROM trending.claims_and_debates(ARRAY(SELECT claim_id FROM listed)) k
      WHERE k.is_claim AND k.is_debate)) v
    WHERE NOT EXISTS (SELECT 1 FROM trending.ranking_pushdowns p WHERE p.entity_id = v.entity_id)
  ),
  -- The one debate-to-claim edge set, used both to count a debate's claims and to find the listed claims it is on:
  -- every claim each placed debate links (listed or not), and itself when it is typed Claim too (listed or not).
  edges AS (
    SELECT d.debate_id, d.claim_id FROM trending.debate_links('{}', ARRAY(SELECT entity_id FROM placed)) d
    UNION
    SELECT k.entity_id, k.entity_id FROM trending.claims_and_debates(ARRAY(SELECT entity_id FROM placed)) k
    WHERE k.is_claim AND k.is_debate
  ),
  -- Ranked debates on listed claims, with how many claims each debate is on.
  on_claim AS (
    SELECT e.claim_id, ls.listed_on, p.entity_id, p.place_value, p.weight,
           count(*) OVER (PARTITION BY e.debate_id) AS claims
    FROM edges e
    JOIN placed p ON p.entity_id = e.debate_id
    LEFT JOIN listed ls ON ls.claim_id = e.claim_id
  ),
  -- Per listed claim, its lead: the best ranked debate on it that is on no other claim.
  leads AS (
    SELECT DISTINCT ON (o.claim_id) o.claim_id, o.entity_id AS lead_id
    FROM on_claim o
    WHERE o.claims = 1 AND o.listed_on IS NOT NULL
    ORDER BY o.claim_id, o.place_value DESC, o.entity_id DESC
  ),
  -- One numbering per claim with a lead: the lead is #1 and lifted, every other ranked debate on it (multi-claim ones
  -- included, whatever their place) is #2.. and dropped. A claim with no lead gets no rows.
  rows AS (
    SELECT o.entity_id, o.claim_id, o.listed_on,
           row_number() OVER (PARTITION BY o.claim_id
                              ORDER BY o.entity_id = ld.lead_id DESC, o.place_value DESC, o.entity_id DESC)::integer
             AS rank_on_claim,
           o.weight
    FROM on_claim o
    JOIN leads ld ON ld.claim_id = o.claim_id
  ),
  boosts AS (
    SELECT r.entity_id, r.claim_id, r.rank_on_claim, r.listed_on,
           CASE WHEN r.rank_on_claim = 1 THEN cfg.agenda_claim_steps ELSE 0 END
             * cfg.ranking_place_step * r.weight * power(cfg.agenda_day_factor, cfg.today - r.listed_on) AS lift,
           CASE WHEN r.rank_on_claim = 1 THEN 0 ELSE cfg.agenda_follower_drop_steps END
             * cfg.ranking_place_step * r.weight * power(cfg.agenda_day_factor, cfg.today - r.listed_on) AS place_drop
    FROM rows r CROSS JOIN cfg
  )
  -- One row per debate: a lift beats a drop (only single-claim debates are lifted, so this only orders a
  -- multi-claim debate's drops), then the larger drop, then the newer listing and the lower claim id.
  SELECT DISTINCT ON (b.entity_id) b.entity_id, b.claim_id, b.rank_on_claim, b.listed_on, b.lift, b.place_drop
  FROM boosts b
  WHERE b.lift > 0 OR b.place_drop > 0
  ORDER BY b.entity_id, b.lift DESC, b.place_drop DESC, b.listed_on DESC, b.claim_id
$$;
--> statement-breakpoint

-- 0124's terms, plus (GEO-3781) the stored agenda lift after the GREATEST and the follower drop on the place value,
-- both only with ranking_on. Everything else is 0124's.
CREATE OR REPLACE FUNCTION trending.entity_trending_terms_as(entity_ids uuid[], ranking_on boolean DEFAULT NULL)
RETURNS TABLE (entity_id uuid, age_credit numeric, trending_score numeric, trending_source text,
               listing_score numeric, last_listed_on date, agenda_id uuid, debates_in_window integer,
               topic_score numeric, topic_id uuid, topic_listed_on date)
LANGUAGE sql STABLE AS $$
  WITH cfg AS (
    SELECT c.*, trending.today() AS today,
           COALESCE(entity_trending_terms_as.ranking_on, c.ranking_places_mode = 2) AS ranking_on
    FROM public.entity_ranking_config c WHERE c.id
  ),
  li AS (SELECT * FROM trending.entity_listings(entity_ids)),
  tp AS (SELECT * FROM trending.entity_topic_listings(entity_ids)),
  rp AS (
    SELECT v.entity_id, v.place_value
    FROM trending.ranking_place_values(entity_ids) v CROSS JOIN cfg
    WHERE cfg.ranking_on
  ),
  pd AS (
    SELECT p.entity_id FROM trending.ranking_pushdowns p CROSS JOIN cfg
    WHERE cfg.ranking_on AND p.entity_id = ANY(entity_ids)
  ),
  -- GEO-3781: the stored agenda lift or drop (only kept while mode 2 is on, but read only with ranking_on).
  ab AS (
    SELECT b.entity_id, b.lift, b.place_drop FROM trending.agenda_boosts b CROSS JOIN cfg
    WHERE cfg.ranking_on AND b.entity_id = ANY(entity_ids)
  ),
  target AS (
    SELECT k.entity_id, k.is_claim,
           CASE WHEN e.created_at ~ '^[0-9]+$' THEN e.created_at::bigint END AS created_epoch
    FROM trending.claims_and_debates(ARRAY(SELECT entity_id FROM li UNION SELECT entity_id FROM tp
                                           UNION SELECT entity_id FROM rp UNION SELECT entity_id FROM pd)) k
    JOIN public.entities e ON e.id = k.entity_id
  ),
  -- Debates in the window on each claim whose own listing counts, for the claim's discount.
  recent AS (
    SELECT d.claim_id, count(DISTINCT d.debate_id)::integer AS debates
    FROM trending.debate_links(ARRAY(SELECT t.entity_id FROM target t JOIN li USING (entity_id)
                                     WHERE t.is_claim AND li.own_claim_listing), '{}') d
    JOIN public.entities de ON de.id = d.debate_id
    CROSS JOIN cfg
    WHERE de.created_at ~ '^[0-9]+$'
      AND de.created_at::bigint >= extract(epoch FROM now()) - cfg.trending_debate_window_days * 86400
    GROUP BY d.claim_id
  ),
  -- A debate's place on its newest ordered day, and how many days ago that was.
  pos AS (
    SELECT p.entity_id, p.position, (SELECT cfg.today FROM cfg) - p.listed_on AS days
    FROM trending.debate_positions(ARRAY(SELECT entity_id FROM li)) p
  ),
  inputs AS (
    SELECT t.entity_id, li.last_listed_on, li.agenda_id, cfg.today - li.last_listed_on AS days,
           CASE WHEN li.entity_id IS NOT NULL THEN
             GREATEST(0, (extract(epoch FROM li.last_listed_on::timestamp AT TIME ZONE trending.time_zone())::numeric
                          - COALESCE(t.created_epoch, 0)) / GREATEST(cfg.tau_seconds, 1)) END AS age_credit,
           CASE WHEN t.is_claim AND li.entity_id IS NOT NULL THEN COALESCE(r.debates, 0) END AS debates,
           pos.position, pos.days AS position_days,
           tp.topic_id, tp.listed_on AS topic_listed_on, cfg.today - tp.listed_on AS topic_days,
           CASE WHEN rp.entity_id IS NULL THEN 0
                ELSE GREATEST(rp.place_value - COALESCE(ab.place_drop, 0),
                              LEAST(rp.place_value, cfg.ranking_place_base)) END AS place_value,
           CASE WHEN rp.entity_id IS NULL THEN 0 ELSE COALESCE(ab.lift, 0) END AS lift,
           pd.entity_id IS NOT NULL AS pushed,
           CASE WHEN pd.entity_id IS NOT NULL THEN cfg.ranking_pushdown ELSE 0 END AS pushdown
    FROM target t
    CROSS JOIN cfg
    LEFT JOIN li ON li.entity_id = t.entity_id
    LEFT JOIN tp ON tp.entity_id = t.entity_id
    LEFT JOIN recent r ON r.claim_id = t.entity_id
    LEFT JOIN pos ON pos.entity_id = t.entity_id
    LEFT JOIN rp ON rp.entity_id = t.entity_id
    LEFT JOIN pd ON pd.entity_id = t.entity_id
    LEFT JOIN ab ON ab.entity_id = t.entity_id
  ),
  terms AS (
    SELECT x.*,
           CASE WHEN x.last_listed_on IS NULL OR x.days > cfg.trending_horizon_days THEN 0
                ELSE cfg.trending_boost * power(cfg.trending_day_factor, x.days)
                                        * power(cfg.trending_debate_factor, COALESCE(x.debates, 0))
                   + CASE WHEN cfg.ranking_on OR x.position IS NULL OR x.position > cfg.trending_position_slots
                               OR x.position_days > cfg.trending_horizon_days THEN 0
                          ELSE (cfg.trending_position_base
                                + cfg.trending_position_step * (cfg.trending_position_slots - x.position + 1))
                               * power(cfg.trending_position_day_factor, x.position_days) END
           END AS listing_score,
           CASE WHEN x.topic_id IS NULL OR x.topic_days > cfg.trending_horizon_days THEN 0
                ELSE cfg.trending_topic_boost * power(cfg.trending_day_factor, x.topic_days) END AS topic_score
    FROM inputs x CROSS JOIN cfg
  )
  SELECT x.entity_id, CASE WHEN x.pushed THEN 0 ELSE COALESCE(x.age_credit, 0) END,
         GREATEST(x.listing_score, x.topic_score, x.place_value) + x.lift - x.pushdown,
         CASE WHEN GREATEST(x.listing_score, x.place_value) > 0
                   AND GREATEST(x.listing_score, x.place_value) >= x.topic_score THEN 'listing'
              WHEN x.topic_score > 0 THEN 'topic' END,
         x.listing_score, x.last_listed_on, x.agenda_id, x.debates,
         x.topic_score, x.topic_id, x.topic_listed_on
  FROM terms x
$$;
--> statement-breakpoint

-- 0124's sweep, plus (GEO-3781) keeping trending.agenda_boosts in step after the places, push-downs and listings are
-- written, and re-scoring the debates whose boost row changed in the same run.
CREATE OR REPLACE FUNCTION trending.refresh_agenda_listings(full_pass boolean DEFAULT NULL)
RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
  claim_ids uuid[];
  debate_ids uuid[];
  topic_ids uuid[];
  moved_ids uuid[];
  ranked_ids uuid[];
  boosted_ids uuid[];
  ids uuid[];
  horizon integer;
  boost numeric;
  step numeric;
  pbase numeric;
  slots integer;
  rmode integer;
  today date;
  scored integer;
BEGIN
  -- One writer at a time; the CronJob forbids overlap too.
  PERFORM pg_advisory_xact_lock(hashtext('trending.agenda_listings'));
  SELECT c.trending_horizon_days, c.trending_boost, c.trending_position_step, c.trending_position_base,
         c.trending_position_slots, c.ranking_places_mode, trending.today()
    INTO horizon, boost, step, pbase, slots, rmode, today
  FROM public.entity_ranking_config c WHERE c.id;
  full_pass := COALESCE(full_pass, (SELECT st.last_full_pass_on IS DISTINCT FROM today
                                    FROM trending.refresh_state st WHERE st.id));
  IF full_pass THEN
    UPDATE trending.refresh_state SET last_full_pass_on = today WHERE id;
  END IF;

  CREATE TEMP TABLE IF NOT EXISTS incoming_agenda_listings (
    agenda_id uuid, space_id uuid, entity_id uuid, listed_on date, source text
  ) ON COMMIT DROP;
  TRUNCATE incoming_agenda_listings;
  INSERT INTO incoming_agenda_listings SELECT * FROM trending.current_agenda_listings();

  CREATE TEMP TABLE IF NOT EXISTS changed_agenda_listings (
    agenda_id uuid, space_id uuid, entity_id uuid, listed_on date, source text, incoming boolean
  ) ON COMMIT DROP;
  TRUNCATE changed_agenda_listings;
  INSERT INTO changed_agenda_listings
  SELECT x.*, false FROM (
    SELECT l.agenda_id, l.space_id, l.entity_id, l.listed_on, l.source FROM trending.agenda_listings l
    EXCEPT SELECT * FROM incoming_agenda_listings) x
  UNION ALL
  SELECT x.*, true FROM (
    SELECT * FROM incoming_agenda_listings
    EXCEPT SELECT l.agenda_id, l.space_id, l.entity_id, l.listed_on, l.source FROM trending.agenda_listings l) x;

  -- Places on the agendas inside the horizon (and later ones). Older rows go: past the horizon a listing
  -- lifts nothing.
  CREATE TEMP TABLE IF NOT EXISTS incoming_debate_positions (
    agenda_id uuid, space_id uuid, entity_id uuid, listed_on date, "position" integer
  ) ON COMMIT DROP;
  TRUNCATE incoming_debate_positions;
  INSERT INTO incoming_debate_positions SELECT * FROM trending.current_agenda_debate_positions(today - horizon);

  CREATE TEMP TABLE IF NOT EXISTS changed_debate_positions (
    agenda_id uuid, space_id uuid, entity_id uuid, listed_on date, "position" integer, incoming boolean
  ) ON COMMIT DROP;
  TRUNCATE changed_debate_positions;
  INSERT INTO changed_debate_positions
  SELECT x.*, false FROM (
    SELECT p.agenda_id, p.space_id, p.entity_id, p.listed_on, p.position FROM trending.agenda_debate_positions p
    EXCEPT SELECT * FROM incoming_debate_positions) x
  UNION ALL
  SELECT x.*, true FROM (
    SELECT * FROM incoming_debate_positions
    EXCEPT SELECT p.agenda_id, p.space_id, p.entity_id, p.listed_on, p.position FROM trending.agenda_debate_positions p) x;

  -- Debates whose place within the slots moved, came or went. Only while the term is on: with a base and
  -- a step of 0 a place changes no score.
  moved_ids := '{}';
  IF step > 0 OR pbase > 0 THEN
    moved_ids := ARRAY(SELECT DISTINCT c.entity_id FROM changed_debate_positions c WHERE c.position <= slots);
  END IF;

  -- GEO-3681: the ranking's places and push-downs, kept only while the mode is not off (off empties them).
  CREATE TEMP TABLE IF NOT EXISTS incoming_ranking_pushdowns (
    entity_id uuid, downvoters integer, rankers integer
  ) ON COMMIT DROP;
  TRUNCATE incoming_ranking_pushdowns;
  CREATE TEMP TABLE IF NOT EXISTS incoming_ranking_places (
    source text, entity_id uuid, place integer
  ) ON COMMIT DROP;
  TRUNCATE incoming_ranking_places;
  IF rmode > 0 THEN
    INSERT INTO incoming_ranking_pushdowns SELECT * FROM trending.current_ranking_pushdowns();
    INSERT INTO incoming_ranking_places
    SELECT * FROM trending.current_ranking_places(ARRAY(SELECT entity_id FROM incoming_ranking_pushdowns));
  END IF;
  -- Debates whose place or push-down came, went or changed. Only while on: in shadow they move no score.
  ranked_ids := '{}';
  IF rmode = 2 THEN
    ranked_ids := ARRAY(
      SELECT x.entity_id FROM (
        (SELECT p.source, p.entity_id, p.place FROM trending.ranking_places p
         EXCEPT SELECT * FROM incoming_ranking_places)
        UNION
        (SELECT * FROM incoming_ranking_places
         EXCEPT SELECT p.source, p.entity_id, p.place FROM trending.ranking_places p)) x
      UNION
      SELECT x.entity_id FROM (
        (SELECT p.entity_id FROM trending.ranking_pushdowns p EXCEPT SELECT entity_id FROM incoming_ranking_pushdowns)
        UNION
        (SELECT entity_id FROM incoming_ranking_pushdowns EXCEPT SELECT p.entity_id FROM trending.ranking_pushdowns p)) x);
  END IF;
  -- A full pass re-scores every debate in either table, before and after: the fade moves with the day and a
  -- weight or mode change applies here.
  IF full_pass THEN
    ranked_ids := ranked_ids || ARRAY(
      SELECT entity_id FROM trending.ranking_places UNION SELECT entity_id FROM incoming_ranking_places
      UNION SELECT entity_id FROM trending.ranking_pushdowns UNION SELECT entity_id FROM incoming_ranking_pushdowns);
  END IF;
  DELETE FROM trending.ranking_places p
  WHERE NOT EXISTS (SELECT 1 FROM incoming_ranking_places i
                    WHERE (i.source, i.entity_id, i.place) = (p.source, p.entity_id, p.place));
  INSERT INTO trending.ranking_places (source, entity_id, place)
  SELECT i.source, i.entity_id, i.place FROM incoming_ranking_places i
  ON CONFLICT (source, entity_id) DO UPDATE SET place = EXCLUDED.place;
  DELETE FROM trending.ranking_pushdowns p
  WHERE NOT EXISTS (SELECT 1 FROM incoming_ranking_pushdowns i WHERE i.entity_id = p.entity_id);
  INSERT INTO trending.ranking_pushdowns (entity_id, downvoters, rankers)
  SELECT i.entity_id, i.downvoters, i.rankers FROM incoming_ranking_pushdowns i
  ON CONFLICT (entity_id) DO UPDATE SET downvoters = EXCLUDED.downvoters, rankers = EXCLUDED.rankers;

  -- Claims, debates and topics whose rows changed (added, removed or re-dated). A full pass adds every
  -- listed claim and debate, for the age credit, and topics inside the horizon: past it a topic lifts
  -- nothing.
  SELECT coalesce(array_agg(DISTINCT x.entity_id) FILTER (WHERE x.source = ANY(trending.claim_sources())
                                                            AND (x.changed OR full_pass)), '{}'),
         coalesce(array_agg(DISTINCT x.entity_id) FILTER (WHERE x.source = 'agenda_debates'
                                                            AND (x.changed OR full_pass)), '{}'),
         coalesce(array_agg(DISTINCT x.entity_id) FILTER (WHERE x.source = 'agenda_topics'
                                                            AND (x.changed OR (full_pass AND x.listed_on BETWEEN today - horizon AND today))), '{}')
    INTO claim_ids, debate_ids, topic_ids
  FROM (
    SELECT c.entity_id, c.source, c.listed_on, true AS changed FROM changed_agenda_listings c
    UNION ALL
    SELECT i.entity_id, i.source, i.listed_on, false FROM incoming_agenda_listings i
  ) x;

  -- Claims listed inside the horizon with a debate that does not carry the boost yet (see 0113's
  -- FRESHNESS). Only while the boost is on: at 0 every such debate would match every hour.
  IF boost > 0 THEN
    claim_ids := claim_ids || ARRAY(
      SELECT DISTINCT dl.claim_id
      FROM trending.debate_links(ARRAY(
        SELECT i.entity_id FROM incoming_agenda_listings i
        WHERE i.source = ANY(trending.claim_sources()) AND i.listed_on BETWEEN today - horizon AND today), '{}') dl
      LEFT JOIN public.entity_ranking_scores s ON s.entity_id = dl.debate_id
      WHERE COALESCE(s.trending_score, 0) = 0);
  END IF;

  DELETE FROM trending.agenda_listings l
  USING changed_agenda_listings c
  WHERE NOT c.incoming
    AND (l.agenda_id, l.space_id, l.entity_id, l.listed_on, l.source)
      = (c.agenda_id, c.space_id, c.entity_id, c.listed_on, c.source);
  INSERT INTO trending.agenda_listings (agenda_id, space_id, entity_id, listed_on, source)
  SELECT c.agenda_id, c.space_id, c.entity_id, c.listed_on, c.source
  FROM changed_agenda_listings c WHERE c.incoming;

  DELETE FROM trending.agenda_debate_positions p
  USING changed_debate_positions c
  WHERE NOT c.incoming
    AND (p.agenda_id, p.space_id, p.entity_id) = (c.agenda_id, c.space_id, c.entity_id);
  INSERT INTO trending.agenda_debate_positions (agenda_id, space_id, entity_id, listed_on, position)
  SELECT c.agenda_id, c.space_id, c.entity_id, c.listed_on, c.position
  FROM changed_debate_positions c WHERE c.incoming;

  -- GEO-3781: the agenda lift and drop, from the places, push-downs and listings just written (one snapshot).
  -- current_agenda_boosts is empty unless mode 2 is on with a step above 0, so off empties the table. Debates whose
  -- row came, went or changed (by full row: rank, claim, Date, lift or drop) are re-scored in this run. Every row is
  -- a placed debate, so a full pass already re-scores them all through ranked_ids.
  CREATE TEMP TABLE IF NOT EXISTS incoming_agenda_boosts (
    entity_id uuid, claim_id uuid, rank_on_claim integer, listed_on date, lift numeric, place_drop numeric
  ) ON COMMIT DROP;
  TRUNCATE incoming_agenda_boosts;
  INSERT INTO incoming_agenda_boosts SELECT * FROM trending.current_agenda_boosts();
  boosted_ids := ARRAY(
    SELECT x.entity_id FROM (
      (SELECT b.entity_id, b.claim_id, b.rank_on_claim, b.listed_on, b.lift, b.place_drop FROM trending.agenda_boosts b
       EXCEPT SELECT * FROM incoming_agenda_boosts)
      UNION
      (SELECT * FROM incoming_agenda_boosts
       EXCEPT SELECT b.entity_id, b.claim_id, b.rank_on_claim, b.listed_on, b.lift, b.place_drop
       FROM trending.agenda_boosts b)) x);
  DELETE FROM trending.agenda_boosts b
  WHERE NOT EXISTS (SELECT 1 FROM incoming_agenda_boosts i WHERE i.entity_id = b.entity_id);
  INSERT INTO trending.agenda_boosts (entity_id, claim_id, rank_on_claim, listed_on, lift, place_drop)
  SELECT * FROM incoming_agenda_boosts
  ON CONFLICT (entity_id) DO UPDATE SET claim_id = EXCLUDED.claim_id, rank_on_claim = EXCLUDED.rank_on_claim,
    listed_on = EXCLUDED.listed_on, lift = EXCLUDED.lift, place_drop = EXCLUDED.place_drop;

  -- Those claims and the debates on them; those debates and the ones whose place moved; claims and
  -- debates tagged with those topics; the ranking's changed debates and agenda boosts; and on a full pass anything
  -- still storing a term (a removed tag or Claims link, a config change), by the partial index.
  SELECT coalesce(array_agg(DISTINCT x.id), '{}') INTO ids
  FROM (
    SELECT unnest(claim_ids) AS id
    UNION ALL
    SELECT d.debate_id FROM trending.debate_links(claim_ids, '{}') d
    UNION ALL
    SELECT unnest(debate_ids)
    UNION ALL
    SELECT unnest(moved_ids)
    UNION ALL
    SELECT unnest(ranked_ids)
    UNION ALL
    SELECT unnest(boosted_ids)
    UNION ALL
    SELECT k.entity_id
    FROM trending.claims_and_debates(ARRAY(
      SELECT tg.from_entity_id FROM public.relations tg
      JOIN public.spaces sp ON sp.id = tg.space_id AND sp.type = 'DAO'
      WHERE tg.to_entity_id = ANY(topic_ids)
        AND tg.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid)) k                -- Topics
    UNION ALL
    SELECT s.entity_id FROM public.entity_ranking_scores s
    WHERE full_pass AND (s.trending_score <> 0 OR s.age_credit <> 0)
  ) x;
  scored := 0;
  IF cardinality(ids) > 0 THEN
    scored := public.refresh_entity_ranking_scores(ids);
  END IF;
  -- The day's shadow report, after the day's scores.
  IF full_pass AND rmode > 0 THEN
    PERFORM trending.write_ranking_shadow_report(today);
  END IF;
  RETURN scored;
END;
$$;
--> statement-breakpoint

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'gaia_app') AND current_user <> 'gaia_app' THEN
    EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON trending.agenda_boosts TO gaia_app';
    EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA trending TO gaia_app';
  END IF;
END
$$;

-- No sweep, no re-score and no config bump here: with both steps at 0 the next hourly run keeps the new table empty
-- and scores exactly as 0124 does.
