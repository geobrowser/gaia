-- GEO-3681: the top of Best can follow the Debates space's Best debates ranking instead of agendas, behind
-- one switch that is OFF by default. Nothing in this migration changes a score: the switch starts at 0
-- (off), the sources table starts empty and no re-score runs here.
--
-- WHY. Since 0122 the head of Best is the agendas' Debates order: place p of the top 50 adds
-- (2500 + 100 × (51 − p)) × 0.3^days to the debate's LISTING score. That term only exists inside the listing
-- and 16-day horizon gates, so a ranked debate that no agenda listed gets nothing, and it fades from the
-- agenda's Date. The Best debates ranking (ranks.ranking_scores for one block) has no date and no listing,
-- so it gets its own place term, read through a small config table of sources.
--
-- WHY NOT SYNTHETIC AGENDA ROWS. Writing the ranking's heads into trending.agenda_listings and
-- trending.agenda_debate_positions (source 'ranking') would reuse 0122's term, but that term has one global
-- day factor and no weight, fades from the listed day and stops at the 16-day horizon; a synthetic listing
-- also carries 0118's boost (8) and its age credit (which moves an old debate's recency up to the listed
-- day), shows as lastListedOn / listedOnAgendaId, and reaches the warehouse export (gaia#1131) and the geo-db
-- collection 20 mirror. Per-source weights and fades by the debate's age, the shadow week at weight 0 and
-- the push-down all need new code either way, so the separate term below is the smaller change.
--
-- THE SWITCH. entity_ranking_config.ranking_places_mode, an integer so the geo-db config mirror can carry it:
--   0 off      (default) the sweep keeps no ranking places, push-downs or reports, and every score is exactly
--              what 0122 gives. The agenda term is unchanged.
--   1 shadow   the sweep keeps the places and push-downs and writes the daily shadow report, but they move no
--              score: the ranking rows are computed at an effective weight of 0. Agendas still set the head.
--   2 on       the ranking places and the push-down are served, and the agenda POSITION term (0122) is
--              dropped: agendas stop setting debate places. Their plain listing boost (0113, 0118) stays.
-- Cutover is 1 → 2 and rollback 2 → 1, each followed by a forced full pass (see the runbook in
-- docs/runbooks/best-debates-ranking-cutover.md). The weights in trending.ranking_place_sources are the ones the
-- report simulates in shadow and the ones served when on.
--
-- SOURCES (trending.ranking_place_sources, one row per source, empty until someone inserts them):
--   space_id, block_id   the ranking block and its space; ranks.ranking_scores rows of that block are read,
--                        deduped by entity (best position) across the item space ids
--   weight               multiplies the place value; a debate placed by several sources keeps the larger
--                        weighted value ("its best weighted place"), values are never added up
--   window_days          only debates created within this many New York days (today included) take a place in
--                        this source, renumbered 1.. among them; NULL means every ranked debate
--   slots                places that carry a value, after renumbering
--   day_factor           the value is multiplied by this for each New York day of the debate's age
--   fade_from            what the fade is measured from; 'debate_created' (the debate's age) is the only basis
--   place value = weight × (ranking_place_base + ranking_place_step × (slots − place + 1)) × day_factor^age_days
-- Only Debate-typed entities that exist in entities and are not on entity_feed_blocklist take places, and a
-- pushed-down debate takes none.
--
-- PUSH-DOWN. For each source's (space, block): members are ranks.members ∪ ranks.editors of the space, keyed
-- by personal space id. Downvoters are the distinct user_votes.user_id in that set with object_type 0,
-- vote_kind 0 (curation), vote_type 1 (down) and voted_at at or after ranking_pushdown_since (epoch seconds;
-- 0 counts nothing). user_votes.space_id is where the vote was cast from, not the voter, and is never read.
-- Rankers are the distinct ranks.rankings.space_id in the same set whose LATEST ballot on the block (the
-- indexer's dedup: newest updated_at_block, then update_index) holds the debate. A Debate-typed debate is
-- pushed down when downvoters > rankers: it takes no place and, while on, its trending score is the
-- GREATEST(listing, topic, place) minus ranking_pushdown, a flat amount however many downvotes there are, and it
-- gets no age credit (an agenda listing it would otherwise move its recency up to the listed day). The agenda
-- position term is already gone for everyone while on. trending_score can therefore be negative for a
-- pushed-down debate; it is the only term that can.
--
-- HOW IT COMBINES. trending_score = GREATEST(listing, topic, ranking place) − push-down. The ranking place is
-- reported as trending_source 'listing' when it wins (a curated list placed it): geo-db's overlay has room for
-- only none / listing / topic. listing_score, last_listed_on and agenda_id stay the agenda's own.
-- entity_trending_terms keeps its signature.
--
-- NUMBERS (defaults; sources are inserted by the runbook). base 2500 and step 100 are 0122's: a place is worth
-- at least 1300 (All time #30 at age 0), far above any organic gap. With the planned rows, New (weight 1,
-- 20 slots, 0.65 a day, last 7 days) places 1..20 at 4500..2600 on the debate's first day, and All time
-- (weight 0.5, 30 slots, 0.97 a day) places 1..30 at 2750..1300 at age 0, about half after 23 days.
-- ranking_pushdown 150: against an unranked debate up to 30 days older, a pushed-down debate starts 57.6 ahead on
-- recency (30 × 86400 / tau at tau 45000) and can carry its listing or topic boost (≤ 8), so it stays below
-- while the rest of its organic lead (quality, intrinsic, participation ≤ 19, comments ≤ 30, watch ≤ 8 + 5
-- fresh, topic ≤ 2) is under 150 − 57.6 − 8 = 84.4. Those caps add up to about 90 at the extreme, so this is
-- not unconditional; the Best top 40 on 9 Nov spanned 5. The shadow report counts any pushed-down debate that
-- would still sit above such a debate (ranking_shadow_days.pushed_above_unranked).
--
-- FRESHNESS. refresh_agenda_listings brings ranking_places and ranking_pushdowns in step every hour (while the
-- mode is not off) and, while on, re-scores the debates whose rows changed. The daily full pass re-scores every
-- debate in either table, which moves each value to the new day's fade and applies a config change; the
-- runbook forces it after a weight or mode change. At the end of a full pass with the mode on or in shadow, the
-- sweep writes the day's shadow report.
--
-- SHADOW REPORT. trending.ranking_shadow_report: per New York day, for every debate in one of three top 50s,
-- its rank in Best today (stored ranking_score DESC, entity_id DESC, Debate-typed, not blocklisted:
-- entities_in_best_order's order), its rank if mode 2 served now (the same order over the score
-- entity_trending_terms_as(ids, true) gives), and its rank by weighted place value alone. trending.ranking_shadow_days
-- holds the overlap of the served and if-on top 50s, the push-down count, the push-down violations and the cutover
-- bar of GEO-3591 (rankers, share of the last 7 days ranked, All time top 30 on 2+ ballots).
-- trending.ranking_shadow_moves lists the biggest moves from served to if-on.
--
-- REVERTS. Any later migration that re-creates entity_trending_terms or refresh_agenda_listings from 0122's bodies
-- (gaia#1483 was one) would silently drop all of this; the 0124 suite fails if either stops reading
-- ranking_places_mode.

ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "ranking_places_mode" integer NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "ranking_place_base" numeric NOT NULL DEFAULT 2500;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "ranking_place_step" numeric NOT NULL DEFAULT 100;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "ranking_pushdown" numeric NOT NULL DEFAULT 150;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "ranking_pushdown_since" bigint NOT NULL DEFAULT 0;
--> statement-breakpoint
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'entity_ranking_config_ranking_places_valid') THEN
    ALTER TABLE "entity_ranking_config" ADD CONSTRAINT "entity_ranking_config_ranking_places_valid"
      CHECK ("ranking_places_mode" IN (0, 1, 2) AND "ranking_place_base" >= 0 AND "ranking_place_step" >= 0
             AND "ranking_pushdown" >= 0 AND "ranking_pushdown_since" >= 0);
  END IF;
END
$$;
--> statement-breakpoint

CREATE TABLE IF NOT EXISTS "trending"."ranking_place_sources" (
  "name" text PRIMARY KEY CHECK ("name" ~ '^[a-z][a-z0-9_]*$'),
  "space_id" uuid NOT NULL,
  "block_id" uuid NOT NULL,
  "weight" numeric NOT NULL CHECK ("weight" >= 0),
  "window_days" integer CHECK ("window_days" IS NULL OR "window_days" > 0),
  "slots" integer NOT NULL CHECK ("slots" > 0),
  "day_factor" numeric NOT NULL CHECK ("day_factor" >= 0 AND "day_factor" <= 1),
  "fade_from" text NOT NULL DEFAULT 'debate_created' CHECK ("fade_from" = 'debate_created'),
  "updated_at" timestamp with time zone NOT NULL DEFAULT now()
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "trending"."ranking_places" (
  "source" text NOT NULL,
  "entity_id" uuid NOT NULL,
  "place" integer NOT NULL,                                            -- 1.. within the source, renumbered
  PRIMARY KEY ("source", "entity_id")
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "ranking_places_entity_idx" ON "trending"."ranking_places" ("entity_id");
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "trending"."ranking_pushdowns" (
  "entity_id" uuid PRIMARY KEY,
  "downvoters" integer NOT NULL,
  "rankers" integer NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "trending"."ranking_shadow_report" (
  "report_on" date NOT NULL,
  "entity_id" uuid NOT NULL,
  "served_rank" integer,                                               -- rank among debates in Best today
  "on_rank" integer,                                                   -- rank among debates if mode 2 served now
  "ranking_rank" integer,                                              -- 1..50 by place value alone, NULL if unplaced
  "ranking_score" numeric,                                             -- the stored Best score when written
  "on_score" numeric,                                                  -- the Best score mode 2 would give
  "place_value" numeric,                                               -- the weighted place value, NULL if unplaced
  "pushed_down" boolean NOT NULL DEFAULT false,
  PRIMARY KEY ("report_on", "entity_id")
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "trending"."ranking_shadow_days" (
  "report_on" date PRIMARY KEY,
  "mode" integer NOT NULL,
  "overlap" integer NOT NULL,                                          -- debates in both top 50s (served, if on)
  "placed" integer NOT NULL,                                           -- debates with a ranking place
  "pushed_down" integer NOT NULL,
  "rankers" integer NOT NULL,                                          -- members with a non-empty latest ballot
  "recent_debates" integer NOT NULL,                                   -- Debate-typed, created in the last 7 days
  "recent_ranked" integer NOT NULL,                                    -- of those, on the block's ranking
  "all_time_top" integer NOT NULL,                                     -- the block's top 30 debates (≤ 30)
  "all_time_top_two_ballots" integer NOT NULL,                         -- of those, on 2+ members' ballots
  "pushed_above_unranked" integer NOT NULL,                            -- pushed-down debates that, if on, would sit
                                                                       -- above an unranked debate created within
                                                                       -- 30 days of them (should be 0)
  "written_at" timestamp with time zone NOT NULL DEFAULT now()
);
--> statement-breakpoint

-- Debates of `ids` that count for a place or a push-down: typed Debate, present in entities, not blocklisted.
CREATE OR REPLACE FUNCTION trending.rankable_debates(ids uuid[])
RETURNS TABLE (entity_id uuid, created_epoch bigint)
LANGUAGE sql STABLE AS $$
  SELECT e.id, CASE WHEN e.created_at ~ '^[0-9]+$' THEN e.created_at::bigint END
  FROM public.entities e
  WHERE e.id = ANY(ids)
    AND EXISTS (SELECT 1 FROM public.relations ty
                WHERE ty.from_entity_id = e.id
                  AND ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid             -- Types
                  AND ty.to_entity_id = 'fd51f935-2063-4617-be39-7b672b23364c'::uuid)       -- Debate
    AND NOT EXISTS (SELECT 1 FROM public.entity_feed_blocklist b WHERE b.entity_id = e.id)
$$;
--> statement-breakpoint

-- A created epoch's New York calendar day.
CREATE OR REPLACE FUNCTION trending.epoch_day(epoch bigint)
RETURNS date LANGUAGE sql STABLE AS $$
  SELECT (to_timestamp(epoch) AT TIME ZONE trending.time_zone())::date
$$;
--> statement-breakpoint

-- The members of each source's (space, block) whose votes and ballots count.
CREATE OR REPLACE FUNCTION trending.ranking_members()
RETURNS TABLE (space_id uuid, block_id uuid, member_space_id uuid)
LANGUAGE sql STABLE AS $$
  SELECT DISTINCT s.space_id, s.block_id, m.member_space_id
  FROM (SELECT DISTINCT space_id, block_id FROM trending.ranking_place_sources) s
  JOIN (SELECT member_space_id, space_id FROM ranks.members
        UNION SELECT member_space_id, space_id FROM ranks.editors) m ON m.space_id = s.space_id
$$;
--> statement-breakpoint

-- Each member's latest ballot on each source block (the indexer's dedup_latest), and what it holds.
CREATE OR REPLACE FUNCTION trending.ranking_member_ballots()
RETURNS TABLE (space_id uuid, block_id uuid, member_space_id uuid, entity_id uuid)
LANGUAGE sql STABLE AS $$
  WITH latest AS (
    SELECT DISTINCT ON (r.block_id, r.space_id) r.id, r.block_id, r.space_id
    FROM ranks.rankings r
    WHERE r.block_id IN (SELECT block_id FROM trending.ranking_place_sources)
    ORDER BY r.block_id, r.space_id, r.updated_at_block DESC, r.update_index DESC, r.id
  )
  SELECT DISTINCT m.space_id, m.block_id, m.member_space_id, ri.entity_id
  FROM latest l
  JOIN trending.ranking_members() m ON m.block_id = l.block_id AND m.member_space_id = l.space_id
  JOIN ranks.ranking_items ri ON ri.ranking_id = l.id
$$;
--> statement-breakpoint

-- Debates pushed down now: more of the members downvoted it (since ranking_pushdown_since) than ranked it.
-- One row per debate; with several (space, block) pairs, the pair with the widest margin (then the most
-- downvoters, then the lower space and block id).
CREATE OR REPLACE FUNCTION trending.current_ranking_pushdowns()
RETURNS TABLE (entity_id uuid, downvoters integer, rankers integer)
LANGUAGE sql STABLE AS $$
  WITH cfg AS (SELECT c.ranking_pushdown_since AS since FROM public.entity_ranking_config c WHERE c.id),
  down AS (
    SELECT m.space_id, m.block_id, uv.object_id AS entity_id, count(DISTINCT uv.user_id)::integer AS n
    FROM trending.ranking_members() m
    JOIN public.user_votes uv ON uv.user_id = m.member_space_id
    CROSS JOIN cfg
    WHERE cfg.since > 0
      AND uv.object_type = 0 AND uv.vote_kind = 0 AND uv.vote_type = 1
      AND uv.voted_at >= to_timestamp(cfg.since)
    GROUP BY 1, 2, 3
  ),
  ranked AS (
    SELECT b.space_id, b.block_id, b.entity_id, count(DISTINCT b.member_space_id)::integer AS n
    FROM trending.ranking_member_ballots() b
    GROUP BY 1, 2, 3
  )
  SELECT DISTINCT ON (d.entity_id) d.entity_id, d.n, COALESCE(r.n, 0)
  FROM down d
  LEFT JOIN ranked r USING (space_id, block_id, entity_id)
  WHERE d.n > COALESCE(r.n, 0)
    AND d.entity_id IN (SELECT rd.entity_id FROM trending.rankable_debates(ARRAY(SELECT entity_id FROM down)) rd)
  ORDER BY d.entity_id, d.n - COALESCE(r.n, 0) DESC, d.n DESC, d.space_id, d.block_id
$$;
--> statement-breakpoint

-- Each source's places now: the block's ranking deduped by entity (best position), rankable and not in
-- `pushed`, inside the window, renumbered 1.. and cut at the slots. Ties on position go to the lower entity id.
-- The sweep passes the push-downs it has just stored, so both come from one read of the votes; NULL (the
-- default, for reading by hand) computes them.
CREATE OR REPLACE FUNCTION trending.current_ranking_places(pushed uuid[] DEFAULT NULL)
RETURNS TABLE (source text, entity_id uuid, place integer)
LANGUAGE sql STABLE AS $$
  WITH ranked AS (
    SELECT s.name, s.window_days, s.slots, rs.entity_id, min(rs.position) AS position
    FROM trending.ranking_place_sources s
    JOIN ranks.ranking_scores rs ON rs.block_id = s.block_id
    GROUP BY s.name, s.window_days, s.slots, rs.entity_id
  ),
  ok AS (
    SELECT rd.entity_id, trending.epoch_day(rd.created_epoch) AS created_on
    FROM trending.rankable_debates(ARRAY(SELECT DISTINCT entity_id FROM ranked)) rd
    WHERE rd.entity_id <> ALL(COALESCE(current_ranking_places.pushed,
                                       ARRAY(SELECT p.entity_id FROM trending.current_ranking_pushdowns() p)))
  ),
  numbered AS (
    SELECT r.name, r.entity_id, r.slots,
           row_number() OVER (PARTITION BY r.name ORDER BY r.position, r.entity_id)::integer AS place
    FROM ranked r
    JOIN ok ON ok.entity_id = r.entity_id
    WHERE r.window_days IS NULL
       OR (ok.created_on IS NOT NULL AND trending.today() - ok.created_on < r.window_days)
  )
  SELECT n.name, n.entity_id, n.place FROM numbered n WHERE n.place <= n.slots
$$;
--> statement-breakpoint

-- Per debate of `entity_ids` with a stored place, its best weighted place value today (whatever the mode).
-- A debate with no numeric created time takes no value: its age is unknown.
CREATE OR REPLACE FUNCTION trending.ranking_place_values(entity_ids uuid[])
RETURNS TABLE (entity_id uuid, place_value numeric)
LANGUAGE sql STABLE AS $$
  WITH cfg AS (SELECT c.*, trending.today() AS today FROM public.entity_ranking_config c WHERE c.id)
  SELECT p.entity_id,
         max(s.weight * (cfg.ranking_place_base + cfg.ranking_place_step * (s.slots - p.place + 1))
             * power(s.day_factor, GREATEST(0, cfg.today - trending.epoch_day(e.created_at::bigint))))
  FROM trending.ranking_places p
  JOIN trending.ranking_place_sources s ON s.name = p.source
  JOIN public.entities e ON e.id = p.entity_id AND e.created_at ~ '^[0-9]+$'
  CROSS JOIN cfg
  WHERE p.entity_id = ANY(entity_ids) AND p.place <= s.slots
  GROUP BY p.entity_id
$$;
--> statement-breakpoint

-- 0122's terms, plus, with `ranking_on`: the ranking place inside the GREATEST, the push-down after it, no
-- agenda position term, and no age credit for a pushed-down debate (an agenda listing it would otherwise move its
-- recency up to the listed day, enough to undo the push-down). Without it, exactly 0122's rows. The shadow report
-- calls it with true to see what mode 2 would serve; entity_trending_terms passes the mode.
CREATE OR REPLACE FUNCTION trending.entity_trending_terms_as(entity_ids uuid[], ranking_on boolean)
RETURNS TABLE (entity_id uuid, age_credit numeric, trending_score numeric, trending_source text,
               listing_score numeric, last_listed_on date, agenda_id uuid, debates_in_window integer,
               topic_score numeric, topic_id uuid, topic_listed_on date)
LANGUAGE sql STABLE AS $$
  WITH cfg AS (
    SELECT c.*, trending.today() AS today, entity_trending_terms_as.ranking_on AS ranking_on
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
           COALESCE(rp.place_value, 0) AS place_value,
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
         GREATEST(x.listing_score, x.topic_score, x.place_value) - x.pushdown,
         CASE WHEN GREATEST(x.listing_score, x.place_value) > 0
                   AND GREATEST(x.listing_score, x.place_value) >= x.topic_score THEN 'listing'
              WHEN x.topic_score > 0 THEN 'topic' END,
         x.listing_score, x.last_listed_on, x.agenda_id, x.debates,
         x.topic_score, x.topic_id, x.topic_listed_on
  FROM terms x
$$;
--> statement-breakpoint

-- What the score reads: the terms as the switch says. Mode 2 (on) serves the ranking; 0 (off) and 1 (shadow)
-- are exactly 0122. Its signature is 0122's.
CREATE OR REPLACE FUNCTION trending.entity_trending_terms(entity_ids uuid[])
RETURNS TABLE (entity_id uuid, age_credit numeric, trending_score numeric, trending_source text,
               listing_score numeric, last_listed_on date, agenda_id uuid, debates_in_window integer,
               topic_score numeric, topic_id uuid, topic_listed_on date)
LANGUAGE sql STABLE AS $$
  SELECT * FROM trending.entity_trending_terms_as(
    entity_ids, (SELECT c.ranking_places_mode = 2 FROM public.entity_ranking_config c WHERE c.id))
$$;
--> statement-breakpoint

-- The day's shadow report: every debate's rank in Best today and if mode 2 served now, its rank by place value
-- alone, for the union of the three top 50s; and the day's overlap, violations and cutover bar. "If on" is the
-- stored ranking_score with the trending term and age credit swapped for entity_trending_terms_as(ids, true), for
-- every debate that carries a term, a place or a push-down (for the rest it is the same). Replaces the day's rows
-- if run twice.
CREATE OR REPLACE FUNCTION trending.write_ranking_shadow_report(day date DEFAULT NULL)
RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
  d date := COALESCE(day, trending.today());
  rows_written integer;
BEGIN
  DELETE FROM trending.ranking_shadow_report WHERE report_on = d;
  DELETE FROM trending.ranking_shadow_days WHERE report_on = d;

  CREATE TEMP TABLE IF NOT EXISTS shadow_debates (
    entity_id uuid PRIMARY KEY, created_epoch bigint, ranking_score numeric, on_score numeric,
    served_rank integer, on_rank integer, ranking_rank integer, place_value numeric,
    pushed boolean, ranked boolean
  ) ON COMMIT DROP;
  TRUNCATE shadow_debates;
  INSERT INTO shadow_debates (entity_id, created_epoch, ranking_score, on_score, pushed, ranked)
  SELECT s.entity_id, rd.created_epoch, s.ranking_score, s.ranking_score,
         EXISTS (SELECT 1 FROM trending.ranking_pushdowns p WHERE p.entity_id = s.entity_id),
         EXISTS (SELECT 1 FROM ranks.ranking_scores rs
                 WHERE rs.entity_id = s.entity_id
                   AND rs.block_id IN (SELECT block_id FROM trending.ranking_place_sources))
  FROM public.entity_ranking_scores s
  JOIN trending.rankable_debates(ARRAY(
         SELECT ty.from_entity_id FROM public.relations ty
         WHERE ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid                    -- Types
           AND ty.to_entity_id = 'fd51f935-2063-4617-be39-7b672b23364c'::uuid)) rd          -- Debate
    ON rd.entity_id = s.entity_id;

  -- Mode 2's terms for the debates whose terms can differ.
  UPDATE shadow_debates sd
  SET on_score = sd.ranking_score - s.trending_score - s.age_credit
                 + COALESCE(t.trending_score, 0) + COALESCE(t.age_credit, 0)
  FROM public.entity_ranking_scores s
  LEFT JOIN trending.entity_trending_terms_as(ARRAY(
              SELECT x.entity_id FROM shadow_debates x
              JOIN public.entity_ranking_scores y USING (entity_id)
              WHERE y.trending_score <> 0 OR y.age_credit <> 0 OR x.pushed
                 OR x.entity_id IN (SELECT entity_id FROM trending.ranking_places)), true) t
    ON t.entity_id = s.entity_id
  WHERE s.entity_id = sd.entity_id
    AND (s.trending_score <> 0 OR s.age_credit <> 0 OR sd.pushed
         OR sd.entity_id IN (SELECT entity_id FROM trending.ranking_places));

  UPDATE shadow_debates sd SET served_rank = r.served, on_rank = r.on_rank
  FROM (SELECT entity_id,
               row_number() OVER (ORDER BY ranking_score DESC, entity_id DESC)::integer AS served,
               row_number() OVER (ORDER BY on_score DESC, entity_id DESC)::integer AS on_rank
        FROM shadow_debates) r
  WHERE r.entity_id = sd.entity_id;

  UPDATE shadow_debates sd SET ranking_rank = a.rk, place_value = a.place_value
  FROM (SELECT pv.entity_id, pv.place_value,
               row_number() OVER (ORDER BY pv.place_value DESC, x.ranking_score DESC NULLS LAST,
                                           pv.entity_id DESC)::integer AS rk
        FROM trending.ranking_place_values(ARRAY(SELECT DISTINCT entity_id FROM trending.ranking_places)) pv
        LEFT JOIN shadow_debates x ON x.entity_id = pv.entity_id) a
  WHERE a.entity_id = sd.entity_id;

  INSERT INTO trending.ranking_shadow_report
    (report_on, entity_id, served_rank, on_rank, ranking_rank, ranking_score, on_score, place_value, pushed_down)
  SELECT d, sd.entity_id, sd.served_rank, sd.on_rank, CASE WHEN sd.ranking_rank <= 50 THEN sd.ranking_rank END,
         sd.ranking_score, sd.on_score, sd.place_value, sd.pushed
  FROM shadow_debates sd
  WHERE sd.served_rank <= 50 OR sd.on_rank <= 50 OR sd.ranking_rank <= 50;
  GET DIAGNOSTICS rows_written = ROW_COUNT;

  WITH ballots AS (SELECT * FROM trending.ranking_member_ballots()),
  blocks AS (SELECT DISTINCT block_id FROM trending.ranking_place_sources),
  scored AS (
    SELECT rs.entity_id, min(rs.position) AS position
    FROM ranks.ranking_scores rs WHERE rs.block_id IN (SELECT block_id FROM blocks)
    GROUP BY rs.entity_id
  ),
  scored_ok AS (
    SELECT sc.entity_id, sc.position
    FROM scored sc JOIN trending.rankable_debates(ARRAY(SELECT entity_id FROM scored)) rd USING (entity_id)
  ),
  top AS (SELECT entity_id FROM scored_ok ORDER BY position, entity_id LIMIT 30),
  recent AS (
    SELECT e.id AS entity_id
    FROM public.entities e
    WHERE e.created_at ~ '^[0-9]+$'
      AND e.id IN (SELECT rd.entity_id FROM trending.rankable_debates(ARRAY(
                     SELECT ty.from_entity_id FROM public.relations ty
                     WHERE ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid
                       AND ty.to_entity_id = 'fd51f935-2063-4617-be39-7b672b23364c'::uuid)) rd)
      AND trending.epoch_day(e.created_at::bigint) BETWEEN d - 6 AND d
  )
  INSERT INTO trending.ranking_shadow_days
    (report_on, mode, overlap, placed, pushed_down, rankers, recent_debates, recent_ranked, all_time_top,
     all_time_top_two_ballots, pushed_above_unranked)
  SELECT d,
         (SELECT c.ranking_places_mode FROM public.entity_ranking_config c WHERE c.id),
         (SELECT count(*) FROM shadow_debates WHERE served_rank <= 50 AND on_rank <= 50)::integer,
         (SELECT count(DISTINCT entity_id) FROM trending.ranking_places)::integer,
         (SELECT count(*) FROM trending.ranking_pushdowns)::integer,
         (SELECT count(DISTINCT member_space_id) FROM ballots)::integer,
         (SELECT count(*) FROM recent)::integer,
         (SELECT count(*) FROM recent WHERE entity_id IN (SELECT entity_id FROM scored))::integer,
         (SELECT count(*) FROM top)::integer,
         (SELECT count(*) FROM top t
          WHERE (SELECT count(DISTINCT b.member_space_id) FROM ballots b WHERE b.entity_id = t.entity_id) >= 2)::integer,
         (SELECT count(*) FROM shadow_debates p
          WHERE p.pushed AND EXISTS (
            SELECT 1 FROM shadow_debates u
            WHERE NOT u.ranked AND NOT u.pushed AND u.on_score < p.on_score
              AND abs(u.created_epoch - p.created_epoch) <= 30 * 86400))::integer;

  RETURN rows_written;
END;
$$;
--> statement-breakpoint

-- The biggest moves of each report: how far each debate moves from Best today to Best if mode 2 served now.
CREATE OR REPLACE VIEW trending.ranking_shadow_moves AS
  SELECT r.report_on, r.entity_id, r.served_rank, r.on_rank, r.ranking_rank,
         r.served_rank - r.on_rank AS move_up,
         r.pushed_down
  FROM trending.ranking_shadow_report r
  ORDER BY r.report_on DESC, abs(r.served_rank - r.on_rank) DESC, r.entity_id;
--> statement-breakpoint

-- 0122's sweep, plus keeping trending.ranking_places and trending.ranking_pushdowns in step while the mode is
-- not off (emptying them when it is), re-scoring the debates whose rows changed while it is on, re-scoring
-- every debate in them on a full pass, and writing the shadow report at the end of a full pass while the mode
-- is not off. Everything else is unchanged.
CREATE OR REPLACE FUNCTION trending.refresh_agenda_listings(full_pass boolean DEFAULT NULL)
RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
  claim_ids uuid[];
  debate_ids uuid[];
  topic_ids uuid[];
  moved_ids uuid[];
  ranked_ids uuid[];
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

  -- Those claims and the debates on them; those debates and the ones whose place moved; claims and
  -- debates tagged with those topics; the ranking's changed debates; and on a full pass anything still
  -- storing a term (a removed tag or Claims link, a config change), by the partial index.
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
    EXECUTE 'GRANT USAGE ON SCHEMA trending TO gaia_app';
    EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON trending.ranking_place_sources, trending.ranking_places,
             trending.ranking_pushdowns, trending.ranking_shadow_report, trending.ranking_shadow_days TO gaia_app';
    EXECUTE 'GRANT SELECT ON trending.ranking_shadow_moves TO gaia_app';
    EXECUTE 'GRANT USAGE ON SCHEMA ranks TO gaia_app';
    EXECUTE 'GRANT SELECT ON ranks.ranking_scores, ranks.rankings, ranks.ranking_items, ranks.members,
             ranks.editors TO gaia_app';
    EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA trending TO gaia_app';
  END IF;
END
$$;

-- No sweep, no re-score and no config bump here: with the mode at 0 the next hourly run keeps the new tables
-- empty and scores exactly as 0122 does.
