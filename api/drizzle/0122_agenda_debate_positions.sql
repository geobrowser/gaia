-- GEO-3606: the order of a Debate agenda's Debates relation sets the order of its debates at the top of
-- Best.
--
-- WHY. 0118 lets an agenda name single debates, and each gets the full listing boost (8). Every named
-- debate gets the same 8, so among themselves they fall back to their organic order (votes, watch
-- time, the fresh lift). A curator can choose which debates reach the top but not which one is first.
--
-- WHAT IT ADDS. A position term on a named debate's listing score:
--
--   position term = position_step × max(0, position_slots − position + 1) × position_day_factor^days
--
--   position   the debate's place in the agenda's Debates relation, numbered 1.. over the targets that
--              count (typed Debate, written in the agenda's own space), as agenda_contents numbers them
--              and currentDebateAgendas returns them (itemPosition). With the debate named by more than
--              one agenda on its newest listing day, the best (lowest) of those positions.
--   days       as the listing's: New York days since that listing day.
--
-- It is added to the listing score, so trending_source stays 'listing' and every reader of
-- trendingScore sees it. Only a debate's own agenda_debates listing carries a position: claim listings,
-- topic listings and debates lifted through their claim are unchanged. A debate named on its newest
-- listing day but whose listing that day came through its claim gets the position too: it was named.
--
-- NUMBERS (defaults, all in entity_ranking_config):
--   trending_position_step        30  per place. The organic spread a step has to beat among debates
--                                     already given the same boost and age credit is about 29 points:
--                                     the watch term's ±8 (16), the fresh lift (5) and the participation
--                                     term's usual range among top debates. The top 40 debates Best
--                                     served on 9 Nov span 5 points, so 30 per place keeps the agenda's
--                                     order even against an outlier.
--   trending_position_slots       20  places that carry a term. Position 1 gets 20 steps (600), position
--                                     20 one step (30), position 21 on only the 0118 boost.
--   trending_position_day_factor   0  today only. power(0, 0) = 1 on the agenda's Date and 0 after it,
--                                     so yesterday's order never sits above today's. A named debate
--                                     keeps 0118's decaying boost after its day. Raise it (up to 1) to
--                                     let an order carry into later days when spaces skip a day.
-- A step of 0 turns the term off.
--
-- STORAGE. trending.agenda_listings has no position and is read as is by the warehouse export (gaia#1131)
-- and by the Rust and TypeScript twins of the listing functions, so positions get their own private table,
-- trending.agenda_debate_positions, kept by the same hourly sweep: one row per agenda, space and named
-- debate, for agendas dated within the horizon (or later). Every function 0118 defined keeps its
-- signature.
--
-- FRESHNESS. refresh_agenda_listings brings the positions table in step every hour and, while the step is
-- on, re-scores the debates whose position changed within the slots: reordering the Debates relation
-- moves Best within the hour. The daily full pass already re-scores every listed debate, which is what
-- ends a term at midnight. A config change waits for the full pass, as every trending term's does.

ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "trending_position_step" numeric NOT NULL DEFAULT 30;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "trending_position_slots" integer NOT NULL DEFAULT 20;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "trending_position_day_factor" numeric NOT NULL DEFAULT 0;
--> statement-breakpoint
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'entity_ranking_config_trending_position_valid') THEN
    ALTER TABLE "entity_ranking_config" ADD CONSTRAINT "entity_ranking_config_trending_position_valid"
      CHECK ("trending_position_step" >= 0 AND "trending_position_slots" >= 0
             AND "trending_position_day_factor" >= 0 AND "trending_position_day_factor" <= 1);
  END IF;
END
$$;
--> statement-breakpoint

CREATE TABLE IF NOT EXISTS "trending"."agenda_debate_positions" (
  "agenda_id" uuid NOT NULL,
  "space_id" uuid NOT NULL,
  "entity_id" uuid NOT NULL,                                           -- the named debate
  "listed_on" date NOT NULL,                                           -- the agenda's Date, New York
  "position" integer NOT NULL,                                         -- 1.. in the Debates relation
  PRIMARY KEY ("agenda_id", "space_id", "entity_id")
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "agenda_debate_positions_entity_idx" ON "trending"."agenda_debate_positions" ("entity_id", "listed_on");
--> statement-breakpoint

-- What the agendas dated since `since` name as debates, with their places. agenda_contents numbers the
-- Debates targets that count, so a target that is not a debate, or written in another space, takes no
-- place.
CREATE OR REPLACE FUNCTION trending.current_agenda_debate_positions(since date DEFAULT NULL)
RETURNS TABLE (agenda_id uuid, space_id uuid, entity_id uuid, listed_on date, "position" integer)
LANGUAGE sql STABLE AS $$
  SELECT c.agenda_id, c.space_id, c.entity_id, c.listed_on, c.item_position
  FROM trending.agenda_contents(NULL, since) c
  WHERE c.source = 'agenda_debates'
$$;
--> statement-breakpoint

-- Per debate in `entity_ids`, its best place on the agendas of `listed_on` (its newest listing day).
CREATE OR REPLACE FUNCTION trending.debate_positions(entity_ids uuid[])
RETURNS TABLE (entity_id uuid, listed_on date, "position" integer)
LANGUAGE sql STABLE AS $$
  SELECT p.entity_id, p.listed_on, min(p.position)::integer
  FROM trending.agenda_debate_positions p
  WHERE p.entity_id = ANY(entity_ids)
  GROUP BY p.entity_id, p.listed_on
$$;
--> statement-breakpoint

-- 0118's terms, plus the position term on the listing score of a debate named on its newest listing
-- day. Everything else is unchanged.
CREATE OR REPLACE FUNCTION trending.entity_trending_terms(entity_ids uuid[])
RETURNS TABLE (entity_id uuid, age_credit numeric, trending_score numeric, trending_source text,
               listing_score numeric, last_listed_on date, agenda_id uuid, debates_in_window integer,
               topic_score numeric, topic_id uuid, topic_listed_on date)
LANGUAGE sql STABLE AS $$
  WITH cfg AS (
    SELECT c.*, trending.today() AS today FROM public.entity_ranking_config c WHERE c.id
  ),
  li AS (SELECT * FROM trending.entity_listings(entity_ids)),
  tp AS (SELECT * FROM trending.entity_topic_listings(entity_ids)),
  target AS (
    SELECT k.entity_id, k.is_claim,
           CASE WHEN e.created_at ~ '^[0-9]+$' THEN e.created_at::bigint END AS created_epoch
    FROM trending.claims_and_debates(ARRAY(SELECT entity_id FROM li UNION SELECT entity_id FROM tp)) k
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
  -- A debate's place on the agendas of its newest listing day, if any named it that day.
  pos AS (
    SELECT p.entity_id, p.position
    FROM trending.debate_positions(ARRAY(SELECT entity_id FROM li)) p
    JOIN li ON li.entity_id = p.entity_id AND li.last_listed_on = p.listed_on
  ),
  inputs AS (
    SELECT t.entity_id, li.last_listed_on, li.agenda_id, cfg.today - li.last_listed_on AS days,
           CASE WHEN li.entity_id IS NOT NULL THEN
             GREATEST(0, (extract(epoch FROM li.last_listed_on::timestamp AT TIME ZONE trending.time_zone())::numeric
                          - COALESCE(t.created_epoch, 0)) / GREATEST(cfg.tau_seconds, 1)) END AS age_credit,
           CASE WHEN t.is_claim AND li.entity_id IS NOT NULL THEN COALESCE(r.debates, 0) END AS debates,
           pos.position,
           tp.topic_id, tp.listed_on AS topic_listed_on, cfg.today - tp.listed_on AS topic_days
    FROM target t
    CROSS JOIN cfg
    LEFT JOIN li ON li.entity_id = t.entity_id
    LEFT JOIN tp ON tp.entity_id = t.entity_id
    LEFT JOIN recent r ON r.claim_id = t.entity_id
    LEFT JOIN pos ON pos.entity_id = t.entity_id
  ),
  terms AS (
    SELECT x.*,
           CASE WHEN x.last_listed_on IS NULL OR x.days > cfg.trending_horizon_days THEN 0
                ELSE cfg.trending_boost * power(cfg.trending_day_factor, x.days)
                                        * power(cfg.trending_debate_factor, COALESCE(x.debates, 0))
                   + CASE WHEN x.position IS NULL OR x.position > cfg.trending_position_slots THEN 0
                          ELSE cfg.trending_position_step * (cfg.trending_position_slots - x.position + 1)
                                                          * power(cfg.trending_position_day_factor, x.days) END
           END AS listing_score,
           CASE WHEN x.topic_id IS NULL OR x.topic_days > cfg.trending_horizon_days THEN 0
                ELSE cfg.trending_topic_boost * power(cfg.trending_day_factor, x.topic_days) END AS topic_score
    FROM inputs x CROSS JOIN cfg
  )
  SELECT x.entity_id, COALESCE(x.age_credit, 0), GREATEST(x.listing_score, x.topic_score),
         CASE WHEN x.listing_score > 0 AND x.listing_score >= x.topic_score THEN 'listing'
              WHEN x.topic_score > 0 THEN 'topic' END,
         x.listing_score, x.last_listed_on, x.agenda_id, x.debates,
         x.topic_score, x.topic_id, x.topic_listed_on
  FROM terms x
$$;
--> statement-breakpoint

-- 0118's sweep, plus keeping trending.agenda_debate_positions in step and, while the position step is on,
-- re-scoring the debates whose place within the slots changed. Everything else is unchanged.
CREATE OR REPLACE FUNCTION trending.refresh_agenda_listings(full_pass boolean DEFAULT NULL)
RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
  claim_ids uuid[];
  debate_ids uuid[];
  topic_ids uuid[];
  moved_ids uuid[];
  ids uuid[];
  horizon integer;
  boost numeric;
  step numeric;
  slots integer;
  today date;
BEGIN
  -- One writer at a time; the CronJob forbids overlap too.
  PERFORM pg_advisory_xact_lock(hashtext('trending.agenda_listings'));
  SELECT c.trending_horizon_days, c.trending_boost, c.trending_position_step, c.trending_position_slots,
         trending.today()
    INTO horizon, boost, step, slots, today
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

  -- Debates whose place within the slots moved, came or went. Only while the step is on: at 0 a place
  -- changes no score.
  moved_ids := '{}';
  IF step > 0 THEN
    moved_ids := ARRAY(SELECT DISTINCT c.entity_id FROM changed_debate_positions c WHERE c.position <= slots);
  END IF;

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

  -- Those claims and the debates on them; those debates and the ones whose place moved; claims and
  -- debates tagged with those topics; and on a full pass anything still storing a term (a removed tag
  -- or Claims link, a config change), by the partial index.
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
  IF cardinality(ids) = 0 THEN
    RETURN 0;
  END IF;
  RETURN public.refresh_entity_ranking_scores(ids);
END;
$$;
--> statement-breakpoint

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'gaia_app') AND current_user <> 'gaia_app' THEN
    EXECUTE 'GRANT USAGE ON SCHEMA trending TO gaia_app';
    EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON trending.agenda_debate_positions TO gaia_app';
    EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA trending TO gaia_app';
  END IF;
END
$$;
--> statement-breakpoint

-- No sweep here: the next hourly run fills the positions table and re-scores the named debates. Bump For
-- you's feed revision (0102) so its analytics can tell the rankings apart.
UPDATE "entity_ranking_config" SET "updated_at" = now() WHERE "id";
