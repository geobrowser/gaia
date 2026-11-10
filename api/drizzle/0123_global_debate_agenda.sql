-- GEO-3631: an agenda in the Debates space sets the order at the top of the whole feed.
--
-- WHY. Since 0122 each space's Debate agenda orders its debates at the top of Best, but every space's place 1
-- gets the same term. In a feed that mixes spaces the first picks tie, and votes and watch time decide which
-- one shows first: on 10 Nov the six 9 Nov agendas' first picks all scored 2255.2. Curators can't set one
-- sequence for the whole feed.
--
-- WHAT IT ADDS. A tier for the global agenda space (the Debates space by default). A place on an agenda in
-- that space adds trending_position_global_base on top of 0122's term:
--
--   position term = (global_base if the agenda is in the global space, else 0
--                    + position_base + position_step × (position_slots − position + 1))
--                   × position_day_factor^days                        for position ≤ position_slots, else 0
--
-- WHICH PLACE. 0122's rule, the debate's newest ordered day, now preferring a place on the global agenda
-- that day over any space agenda's: a debate the Debates agenda places 5th and its space agenda 1st ranks
-- 5th in the Debates order, above every space agenda's debates.
--
-- NUMBERS (defaults; 0122's base 2500, step 100, slots 50, day factor 0.3 unchanged):
--   trending_global_agenda_space_id   the Debates space (9284477c…). NULL turns the tier off.
--   trending_position_global_base     10000. Today's last global place (10000 + 2500 + 100 = 12600) sits above
--                                     today's first space place (7500) by 5100, far more than the ~90-point
--                                     organic ceiling 0122 sizes against. A base of 0 also turns it off.
-- So the head of Best is: today's Debates agenda in its order, then today's space agendas (each in its own
-- order, interleaved by place), then yesterday's.
--
-- FADE. The tier fades with the rest of the term, 0.3 a day. If no Debates agenda is dated today, yesterday's
-- Debates order (3780 to 5250) sits below the first ~23 places of today's space agendas and above the rest.
-- Publishing a Debates agenda each day keeps it on top.
--
-- FRESHNESS. Nothing new to sweep: the positions table already holds every agenda's places, the global
-- space included. The hourly run re-scores moved places as before (and now while only the global base is
-- on), and a config change waits for the daily full pass, as every trending term's does.

ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "trending_global_agenda_space_id" uuid DEFAULT '9284477c-8fe7-4636-8698-994e2e517b32';
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "trending_position_global_base" numeric NOT NULL DEFAULT 10000;
--> statement-breakpoint
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'entity_ranking_config_trending_position_global_valid') THEN
    ALTER TABLE "entity_ranking_config" ADD CONSTRAINT "entity_ranking_config_trending_position_global_valid"
      CHECK ("trending_position_global_base" >= 0);
  END IF;
END
$$;
--> statement-breakpoint

-- 0122's function, plus is_global, and on the newest ordered day the global agenda's place first. The return
-- type changes, so drop and create.
DROP FUNCTION IF EXISTS trending.debate_positions(uuid[]);
--> statement-breakpoint
CREATE FUNCTION trending.debate_positions(entity_ids uuid[])
RETURNS TABLE (entity_id uuid, listed_on date, "position" integer, is_global boolean)
LANGUAGE sql STABLE AS $$
  SELECT DISTINCT ON (p.entity_id) p.entity_id, p.listed_on, p.position,
         COALESCE(p.space_id = c.trending_global_agenda_space_id, false) AS is_global
  FROM trending.agenda_debate_positions p
  CROSS JOIN public.entity_ranking_config c
  WHERE c.id AND p.entity_id = ANY(entity_ids) AND p.listed_on <= trending.today()
  ORDER BY p.entity_id, p.listed_on DESC, COALESCE(p.space_id = c.trending_global_agenda_space_id, false) DESC,
           p.position
$$;
--> statement-breakpoint

-- 0122's terms, plus the global base on a place from the global agenda. Everything else is unchanged.
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
  -- A debate's place on its newest ordered day (the global agenda's place first), and how many days ago
  -- that was.
  pos AS (
    SELECT p.entity_id, p.position, p.is_global, (SELECT cfg.today FROM cfg) - p.listed_on AS days
    FROM trending.debate_positions(ARRAY(SELECT entity_id FROM li)) p
  ),
  inputs AS (
    SELECT t.entity_id, li.last_listed_on, li.agenda_id, cfg.today - li.last_listed_on AS days,
           CASE WHEN li.entity_id IS NOT NULL THEN
             GREATEST(0, (extract(epoch FROM li.last_listed_on::timestamp AT TIME ZONE trending.time_zone())::numeric
                          - COALESCE(t.created_epoch, 0)) / GREATEST(cfg.tau_seconds, 1)) END AS age_credit,
           CASE WHEN t.is_claim AND li.entity_id IS NOT NULL THEN COALESCE(r.debates, 0) END AS debates,
           pos.position, pos.is_global, pos.days AS position_days,
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
                   + CASE WHEN x.position IS NULL OR x.position > cfg.trending_position_slots
                               OR x.position_days > cfg.trending_horizon_days THEN 0
                          ELSE (CASE WHEN x.is_global THEN cfg.trending_position_global_base ELSE 0 END
                                + cfg.trending_position_base
                                + cfg.trending_position_step * (cfg.trending_position_slots - x.position + 1))
                               * power(cfg.trending_position_day_factor, x.position_days) END
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

-- 0122's sweep; the only change is that the global base also turns on re-scoring moved places.
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
  pbase numeric;
  gbase numeric;
  slots integer;
  today date;
BEGIN
  -- One writer at a time; the CronJob forbids overlap too.
  PERFORM pg_advisory_xact_lock(hashtext('trending.agenda_listings'));
  SELECT c.trending_horizon_days, c.trending_boost, c.trending_position_step, c.trending_position_base,
         c.trending_position_global_base, c.trending_position_slots, trending.today()
    INTO horizon, boost, step, pbase, gbase, slots, today
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

  -- Debates whose place within the slots moved, came or went. Only while the term is on: with every base
  -- and the step at 0 a place changes no score.
  moved_ids := '{}';
  IF step > 0 OR pbase > 0 OR gbase > 0 THEN
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
    EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA trending TO gaia_app';
  END IF;
END
$$;
--> statement-breakpoint

-- No sweep here: the next daily full pass re-scores every listed debate with the tier, and the hourly run
-- re-scores any whose place moves before then. Bump For you's feed revision (0102).
UPDATE "entity_ranking_config" SET "updated_at" = now() WHERE "id";
