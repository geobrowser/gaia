-- GEO-3488: Best ranks what each space's Debate agenda names: its claims, the debates recorded on
-- them, and, more lightly, everything on its trending topics.
--
-- WHY. Best is mostly recency plus vote volume, so a claim put on today's agenda with no votes misses
-- the top of every ranked list, and a year-old claim we list today is buried by its age, along with
-- the debates already recorded on it. Each debate space now publishes a daily Debate agenda
-- (GEO-3487) naming the claims and topics we want debated today.
--
-- WHAT AN AGENDA NAMES. A Debate agenda (type 79e3757b) in space S has a Date (52665f3e, a datetime
-- written as midnight New York) in S. It names:
--
--   claims  through Blocks (beaba5cb) → data block → Collection item (a99f9ce1) → claim, and through
--           its own Claims relation (e614cce1) → claim. Both are listings and rank the same way; only
--           block items show in the app. Targets must be typed Claim.
--   topics  through its own Topics relation (806d52bc) → topic. A data block's Topics relation only
--           labels the block and boosts nothing.
--
-- Every relation and the Date must be written in S, the agenda's own space. Every published agenda
-- counts, including an earlier one for the same space and Date; the newer one is only the freshest.
-- An agenda counts once its Date arrives in New York, so tomorrow's agenda published tonight does
-- nothing until midnight. The relations table has no timestamps and nothing re-scores an entity when
-- a relation changes, so every hour what agendas name is read from the graph and the private table
-- trending.agenda_listings brought in step with it (an edited or deleted agenda is handled for free).
--
-- THE LISTINGS TABLE is also read outside ranking, so each row says how the agenda named it:
-- source 'block_item' (what the app shows as the agenda's contents), 'agenda_claims' (the agenda's
-- own Claims relation) or 'agenda_topics' (a topic listing, which lifts tagged entities but is not a
-- claim listing: the Trending tag, lastListedOn and "debates on an agenda claim" ignore it). Its shape
-- is kept flat for the warehouse export: agenda_id, space_id (the agenda's), entity_id (the claim,
-- or the topic for a topic listing), listed_on (the agenda's Date), source, written_at (when the
-- hourly sweep first saw the row, so up to an hour after publishing; unchanged rows keep it).
--
-- THE TERMS, both already included in ranking_score, in score units (one unit is tau, 100000 s,
-- about 28 hours of recency):
--
--   listing  claim   trending  = boost × day_factor^days × debate_factor^debates_in_window
--                    age_credit = max(0, (midnight of newest listing − created_at) / tau)
--            debate  trending  = boost × day_factor^days                (its claim's days)
--                    age_credit = max(0, (midnight of its claim's newest listing − created_at) / tau)
--   topic    claim or debate tagged with a topic an agenda names:
--                    trending  = topic_boost × day_factor^days          (the topic's days)
--                    no age credit, no debate discount
--
--   days               whole New York days since the newest arrived agenda that names the claim
--                      (or, for a topic, the topic). With several claims or topics, the newest.
--   debates_in_window  debates on the claim (typed Debate, Claims relation) created in the last
--                      trending_debate_window_days (7). News stories carry Claims relations too and
--                      are not debates.
--
-- An entity that qualifies for both a listing and a topic boost gets the larger, not both;
-- trending_source and trending_topic_id record which one applied. The age credit comes only from a
-- listing, whichever term won.
--
-- Every debate on a listed claim gets the boost and the credit, including debates recorded before
-- the listing: listing an old claim that was already debated brings its debate back. All debates on
-- one claim get the same boost, and the watch term (0111) separates them. The credit ranks a debate
-- as if published on its claim's listing day; a debate recorded after the listing keeps its real age
-- (the credit is 0).
--
-- The age credit is permanent: it is read from the listings table on every re-score, including the
-- one each vote triggers, so it survives everything except the agenda being removed. The boost decays
-- with no product cutoff. For bookkeeping it is exactly 0 once `days` passes trending_horizon_days
-- (16), where 8 × 0.65^16 = 0.008, so the hourly sweep set stays bounded by construction.
--
-- NUMBERS (decided 28 Oct from the calibration against the live Best feed). The top 117 claims Best
-- serves span only 5.2 points, so a boost of 8 fills every visible claim slot with today's agenda
-- even with no votes, while votes still order agenda claims among themselves. Debate engagement per
-- impression falls about a third a day; 0.65 keeps the boost a little longer, since over-boosting is
-- preferred to under-boosting. A topic boost of 2.5 lifts a median topic item into view and keeps it
-- under the agenda. Each debate in the past week takes a quarter off a claim's boost, less than one
-- day of decay. Two debate counts, on purpose: this discount counts debates in the past 7 days,
-- while the app's debate count counts debates since the last listing, from geo-chat.
--
-- All the numbers are configuration in entity_ranking_config. A boost of 0 turns that boost off; the
-- age credit has no switch of its own because it only moves an entity to the date we listed it. The
-- calendar is fixed to New York (trending.time_zone()): a bad zone in config would fail every re-score.
--
-- FRESHNESS. The terms live in refresh_entity_ranking_scores, so every re-score applies them.
-- trending.refresh_agenda_listings(), run hourly by ranking-indexer's agenda_listings_refresh,
-- updates the listings table and re-scores, in the same transaction: every claim with a claim
-- listing (listed or removed) and the debates on it, the claims and debates tagged with topics whose
-- listings changed or are inside the horizon, and anything still storing a non-zero term (a removed
-- tag or Claims link, a config change), found through a partial index. A vote racing the sweep can
-- write terms from before the sweep; the next hourly run corrects them.
--
-- API. Entity gets computed fields: trendingScore, trendingAgeCredit, trendingSource ('listing' or
-- 'topic') and trendingTopicId are what the stored score used; lastListedOn, listedOnAgendaId and
-- trendingTopicListedOn are live (for a debate, its claim's listing). The table lives in the private
-- `trending` schema because PostGraphile publishes every public table.

CREATE SCHEMA IF NOT EXISTS "trending";
--> statement-breakpoint

ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "trending_boost" numeric NOT NULL DEFAULT 8;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "trending_topic_boost" numeric NOT NULL DEFAULT 2.5;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "trending_day_factor" numeric NOT NULL DEFAULT 0.65;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "trending_debate_factor" numeric NOT NULL DEFAULT 0.75;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "trending_debate_window_days" integer NOT NULL DEFAULT 7;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "trending_horizon_days" integer NOT NULL DEFAULT 16;
--> statement-breakpoint
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'entity_ranking_config_trending_terms_valid') THEN
    ALTER TABLE "entity_ranking_config" ADD CONSTRAINT "entity_ranking_config_trending_terms_valid"
      CHECK ("trending_boost" >= 0 AND "trending_topic_boost" >= 0
             AND "trending_day_factor" > 0 AND "trending_day_factor" <= 1
             AND "trending_debate_factor" > 0 AND "trending_debate_factor" <= 1
             AND "trending_debate_window_days" > 0 AND "trending_horizon_days" >= 0);
  END IF;
END
$$;
--> statement-breakpoint

-- Stored for explainability, like the other terms; the two numbers are already included in
-- ranking_score. Constant defaults, so adding them rewrites no rows.
ALTER TABLE "entity_ranking_scores"
  ADD COLUMN IF NOT EXISTS "age_credit" numeric NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE "entity_ranking_scores"
  ADD COLUMN IF NOT EXISTS "trending_score" numeric NOT NULL DEFAULT 0;
--> statement-breakpoint
-- 'listing' or 'topic': which boost trending_score is. NULL when neither is above 0.
ALTER TABLE "entity_ranking_scores"
  ADD COLUMN IF NOT EXISTS "trending_source" text;
--> statement-breakpoint
-- The topic behind a 'topic' trending_score.
ALTER TABLE "entity_ranking_scores"
  ADD COLUMN IF NOT EXISTS "trending_topic_id" uuid;
--> statement-breakpoint
-- The hourly sweep re-scores everything still carrying a term, so that set must not be a full scan.
-- Large envs: add the columns above and build this CONCURRENTLY first (then the IF NOT EXISTS below
-- no-ops):
--   SET lock_timeout=0; SET statement_timeout=0;
--   CREATE INDEX CONCURRENTLY IF NOT EXISTS "entity_ranking_scores_trending_idx" ON "entity_ranking_scores" ("entity_id") WHERE "trending_score" <> 0 OR "age_credit" <> 0;
CREATE INDEX IF NOT EXISTS "entity_ranking_scores_trending_idx" ON "entity_ranking_scores" ("entity_id")
  WHERE "trending_score" <> 0 OR "age_credit" <> 0;
--> statement-breakpoint

-- Every claim and topic an agenda names, one row per way it names it, future-dated agendas included
-- (readers skip those until their Date arrives). Kept in step with the graph by
-- trending.refresh_agenda_listings(); see THE LISTINGS TABLE above.
CREATE TABLE IF NOT EXISTS "trending"."agenda_listings" (
  "agenda_id" uuid NOT NULL,
  "space_id" uuid NOT NULL,
  "entity_id" uuid NOT NULL,
  "listed_on" date NOT NULL,
  "source" text NOT NULL CHECK ("source" IN ('block_item', 'agenda_claims', 'agenda_topics')),
  "written_at" timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY ("agenda_id", "space_id", "entity_id", "source")
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "agenda_listings_entity_idx" ON "trending"."agenda_listings" ("entity_id");
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "agenda_listings_listed_on_idx" ON "trending"."agenda_listings" ("listed_on");
--> statement-breakpoint

-- New York: agenda Dates, "today" and day counts are all read in this calendar.
CREATE OR REPLACE FUNCTION trending.time_zone()
RETURNS text LANGUAGE sql IMMUTABLE AS $$ SELECT 'America/New_York' $$;
--> statement-breakpoint
CREATE OR REPLACE FUNCTION trending.today()
RETURNS date LANGUAGE sql STABLE AS $$ SELECT (now() AT TIME ZONE trending.time_zone())::date $$;
--> statement-breakpoint

-- A Date value's instant. datetime_utc is what the indexer writes; the text column is the fallback,
-- parsed defensively so one malformed value cannot fail the hourly sweep. Text with no offset is New
-- York wall time, not the session's zone.
CREATE OR REPLACE FUNCTION trending.value_instant(datetime_utc timestamptz, datetime text)
RETURNS timestamptz
LANGUAGE plpgsql STABLE AS $$
BEGIN
  IF datetime_utc IS NOT NULL THEN
    RETURN datetime_utc;
  END IF;
  IF datetime IS NULL OR datetime !~ '^\d{4}-\d{2}-\d{2}' THEN
    RETURN NULL;
  END IF;
  IF datetime ~ '(Z|z|[+-]\d{2}(:?\d{2})?)$' AND datetime ~ 'T|\s' THEN
    RETURN datetime::timestamptz;
  END IF;
  RETURN datetime::timestamp AT TIME ZONE trending.time_zone();
EXCEPTION WHEN others THEN
  RETURN NULL;
END;
$$;
--> statement-breakpoint

-- Which of `ids` are typed Claim and which Debate (an entity can be both); nothing for the rest.
CREATE OR REPLACE FUNCTION trending.claims_and_debates(ids uuid[])
RETURNS TABLE (entity_id uuid, is_claim boolean, is_debate boolean)
LANGUAGE sql STABLE AS $$
  SELECT ty.from_entity_id,
         bool_or(ty.to_entity_id = '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid),       -- Claim
         bool_or(ty.to_entity_id = 'fd51f935-2063-4617-be39-7b672b23364c'::uuid)        -- Debate
  FROM public.relations ty
  WHERE ty.from_entity_id = ANY(ids)
    AND ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid                      -- Types
    AND ty.to_entity_id IN ('96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid,
                            'fd51f935-2063-4617-be39-7b672b23364c'::uuid)
  GROUP BY ty.from_entity_id
$$;
--> statement-breakpoint

-- The debates recorded on `claim_ids`: entities typed Debate with a Claims relation to the claim, in
-- any space. News stories carry Claims relations too and are not debates.
CREATE OR REPLACE FUNCTION trending.debates_on(claim_ids uuid[])
RETURNS TABLE (debate_id uuid, claim_id uuid)
LANGUAGE sql STABLE AS $$
  WITH linked AS (
    SELECT DISTINCT cl.from_entity_id AS debate_id, cl.to_entity_id AS claim_id
    FROM public.relations cl
    WHERE cl.to_entity_id = ANY(claim_ids)
      AND cl.type_id = 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid                    -- Claims
  )
  SELECT l.debate_id, l.claim_id
  FROM linked l
  JOIN trending.claims_and_debates(ARRAY(SELECT debate_id FROM linked)) k
    ON k.entity_id = l.debate_id AND k.is_debate
$$;
--> statement-breakpoint

-- Every agenda as the graph has it now: one row per agenda and space it is typed in, with its Date
-- there (if an edit ever leaves two, the later one).
CREATE OR REPLACE FUNCTION trending.current_agendas()
RETURNS TABLE (agenda_id uuid, space_id uuid, listed_on date)
LANGUAGE sql STABLE AS $$
  SELECT ty.from_entity_id, ty.space_id,
         max((trending.value_instant(v.datetime_utc, v.datetime) AT TIME ZONE trending.time_zone())::date)
  FROM public.relations ty
  JOIN public.values v
    ON v.entity_id = ty.from_entity_id AND v.space_id = ty.space_id
   AND v.property_id = '52665f3e-fb7d-48d5-8b6b-abb21b0d36db'::uuid                 -- Date
  WHERE ty.to_entity_id = '79e3757b-2630-492c-9d3b-d73a78d45893'::uuid             -- Debate agenda
    AND ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid                  -- Types
  GROUP BY ty.from_entity_id, ty.space_id
  HAVING max(trending.value_instant(v.datetime_utc, v.datetime)) IS NOT NULL
$$;
--> statement-breakpoint

-- What agendas name now, in their own space: block items and the agenda's own Claims relation (both
-- typed Claim), and the agenda's own Topics relation.
CREATE OR REPLACE FUNCTION trending.current_agenda_listings()
RETURNS TABLE (agenda_id uuid, space_id uuid, entity_id uuid, listed_on date, source text)
LANGUAGE sql STABLE AS $$
  WITH agendas AS (SELECT * FROM trending.current_agendas()),
  claims AS (
    SELECT a.agenda_id, a.space_id, item.to_entity_id AS entity_id, a.listed_on, 'block_item' AS source
    FROM agendas a
    JOIN public.relations blk
      ON blk.from_entity_id = a.agenda_id AND blk.space_id = a.space_id
     AND blk.type_id = 'beaba5cb-a677-41a8-b353-77030613fc70'::uuid                 -- Blocks
    JOIN public.relations item
      ON item.from_entity_id = blk.to_entity_id AND item.space_id = a.space_id
     AND item.type_id = 'a99f9ce1-2ffa-4dac-8c61-f6310d46064a'::uuid                -- Collection item
    UNION
    SELECT a.agenda_id, a.space_id, cl.to_entity_id, a.listed_on, 'agenda_claims'
    FROM agendas a
    JOIN public.relations cl
      ON cl.from_entity_id = a.agenda_id AND cl.space_id = a.space_id
     AND cl.type_id = 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid                  -- Claims
  )
  SELECT c.agenda_id, c.space_id, c.entity_id, c.listed_on, c.source
  FROM claims c
  JOIN trending.claims_and_debates(ARRAY(SELECT DISTINCT entity_id FROM claims)) k
    ON k.entity_id = c.entity_id AND k.is_claim
  UNION
  SELECT a.agenda_id, a.space_id, tp.to_entity_id, a.listed_on, 'agenda_topics'
  FROM agendas a
  JOIN public.relations tp
    ON tp.from_entity_id = a.agenda_id AND tp.space_id = a.space_id
   AND tp.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid                    -- Topics
$$;
--> statement-breakpoint

-- Per claim, its newest arrived claim listing (topic listings never count) and on which agenda and
-- space.
CREATE OR REPLACE FUNCTION trending.claim_listings(claim_ids uuid[])
RETURNS TABLE (claim_id uuid, last_listed_on date, agenda_id uuid, space_id uuid)
LANGUAGE sql STABLE AS $$
  SELECT DISTINCT ON (l.entity_id) l.entity_id, l.listed_on, l.agenda_id, l.space_id
  FROM trending.agenda_listings l
  WHERE l.entity_id = ANY(claim_ids) AND l.source <> 'agenda_topics'
    AND l.listed_on <= trending.today()
  ORDER BY l.entity_id, l.listed_on DESC, l.agenda_id
$$;
--> statement-breakpoint

-- Per claim or debate in `entity_ids`, the newest arrived claim listing that counts for it: its own
-- if it is a claim, its Claims' if it is a debate.
CREATE OR REPLACE FUNCTION trending.entity_listings(entity_ids uuid[])
RETURNS TABLE (entity_id uuid, last_listed_on date, agenda_id uuid)
LANGUAGE sql STABLE AS $$
  WITH k AS (SELECT * FROM trending.claims_and_debates(entity_ids)),
  claim_of AS (
    SELECT k.entity_id, k.entity_id AS claim_id FROM k WHERE k.is_claim
    UNION
    SELECT k.entity_id, cl.to_entity_id
    FROM k
    JOIN public.relations cl
      ON cl.from_entity_id = k.entity_id AND cl.type_id = 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid -- Claims
    WHERE k.is_debate AND NOT k.is_claim
  )
  SELECT DISTINCT ON (co.entity_id) co.entity_id, l.last_listed_on, l.agenda_id
  FROM claim_of co
  JOIN trending.claim_listings(ARRAY(SELECT claim_id FROM claim_of)) l ON l.claim_id = co.claim_id
  ORDER BY co.entity_id, l.last_listed_on DESC, l.agenda_id
$$;
--> statement-breakpoint

-- Per claim or debate in `entity_ids`, the newest arrived topic listing among its own Topics (in any
-- space).
CREATE OR REPLACE FUNCTION trending.entity_topic_listings(entity_ids uuid[])
RETURNS TABLE (entity_id uuid, topic_id uuid, listed_on date)
LANGUAGE sql STABLE AS $$
  SELECT DISTINCT ON (k.entity_id) k.entity_id, tl.entity_id, tl.listed_on
  FROM trending.claims_and_debates(entity_ids) k
  JOIN public.relations tr
    ON tr.from_entity_id = k.entity_id AND tr.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid -- Topics
  JOIN trending.agenda_listings tl
    ON tl.entity_id = tr.to_entity_id AND tl.source = 'agenda_topics' AND tl.listed_on <= trending.today()
  ORDER BY k.entity_id, tl.listed_on DESC, tl.entity_id
$$;
--> statement-breakpoint

-- The terms for those of `entity_ids` that an agenda boosts: listed claims, debates on them, and
-- claims and debates tagged with a topic an agenda names. One row per such entity; nothing for
-- anything else.
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
  -- Debates in the window on each listed claim, for the claim's discount.
  recent AS (
    SELECT d.claim_id, count(DISTINCT d.debate_id)::integer AS debates
    FROM trending.debates_on(ARRAY(SELECT t.entity_id FROM target t JOIN li USING (entity_id) WHERE t.is_claim)) d
    JOIN public.entities de ON de.id = d.debate_id
    CROSS JOIN cfg
    WHERE de.created_at ~ '^[0-9]+$'
      AND de.created_at::bigint >= extract(epoch FROM now()) - cfg.trending_debate_window_days * 86400
    GROUP BY d.claim_id
  ),
  inputs AS (
    SELECT t.entity_id, li.last_listed_on, li.agenda_id, cfg.today - li.last_listed_on AS days,
           CASE WHEN li.entity_id IS NOT NULL THEN
             GREATEST(0, (extract(epoch FROM li.last_listed_on::timestamp AT TIME ZONE trending.time_zone())::numeric
                          - COALESCE(t.created_epoch, 0)) / GREATEST(cfg.tau_seconds, 1)) END AS age_credit,
           CASE WHEN t.is_claim AND li.entity_id IS NOT NULL THEN COALESCE(r.debates, 0) END AS debates,
           tp.topic_id, tp.listed_on AS topic_listed_on, cfg.today - tp.listed_on AS topic_days
    FROM target t
    CROSS JOIN cfg
    LEFT JOIN li ON li.entity_id = t.entity_id
    LEFT JOIN tp ON tp.entity_id = t.entity_id
    LEFT JOIN recent r ON r.claim_id = t.entity_id
  ),
  terms AS (
    SELECT x.*,
           CASE WHEN x.last_listed_on IS NULL OR x.days > cfg.trending_horizon_days THEN 0
                ELSE cfg.trending_boost * power(cfg.trending_day_factor, x.days)
                                        * power(cfg.trending_debate_factor, COALESCE(x.debates, 0)) END AS listing_score,
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

-- The score choke point as 0111 left it, plus the age credit and the trending term (with which
-- boost it is). Everything else is unchanged.
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
  -- GEO-3488: rows only for claims and debates an agenda boosts; everything else gets 0 below.
  trend AS (
    SELECT ett.entity_id AS id, ett.age_credit, ett.trending_score, ett.trending_source,
           CASE WHEN ett.trending_source = 'topic' THEN ett.topic_id END AS trending_topic_id
    FROM trending.entity_trending_terms(entity_ids) ett
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
           COALESCE(tr.age_credit, 0) AS age_credit,
           COALESCE(tr.trending_score, 0) AS trending_score,
           tr.trending_source,
           tr.trending_topic_id,
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
    LEFT JOIN trend tr   ON tr.id = t.id
  )
  INSERT INTO entity_ranking_scores AS s
    (entity_id, quality_score, intrinsic_score, participation_score, comment_score, ranking_score,
     positive, negative, stance_positive, stance_negative, commenter_count, type_weight, topic_score,
     watch_score, age_credit, trending_score, trending_source, trending_topic_id, updated_at)
  SELECT c.id, c.quality_score, c.intrinsic_score, c.participation_score, c.comment_score,
         -- Not a topic debate: exactly 0098's expression, plus the watch term (0 if not a debate),
         -- plus the age credit and trending term (0 unless an agenda boosts the entity).
         CASE WHEN c.topic_score IS NULL
              THEN public.entity_ranking_score(c.quality_score, c.type_weight, c.intrinsic_score,
                                                c.participation_score, c.comment_score, c.created_epoch,
                                                cfg.tau_seconds, cfg.quality_floor)
              ELSE public.entity_ranking_score(c.quality_score, c.type_weight, c.intrinsic_score,
                                                c.participation_score, c.comment_score, c.created_epoch,
                                                cfg.tau_seconds, cfg.quality_floor)
                   + c.topic_score
         END + c.watch_score + c.age_credit + c.trending_score,
         c.positive, c.negative, c.stance_positive, c.stance_negative, c.commenter_count,
         c.type_weight, COALESCE(c.topic_score, 0), c.watch_score, c.age_credit, c.trending_score,
         c.trending_source, c.trending_topic_id, now()
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
    age_credit          = EXCLUDED.age_credit,
    trending_score      = EXCLUDED.trending_score,
    trending_source     = EXCLUDED.trending_source,
    trending_topic_id   = EXCLUDED.trending_topic_id,
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


-- Brings trending.agenda_listings in step with the graph and re-scores every entity whose terms can
-- have moved. Rows that did not change are left alone, so they keep their written_at; a re-dated row
-- is a new row. One call is one transaction: readers keep the previous rows and scores until it
-- commits. Run hourly by ranking-indexer's agenda_listings_refresh. Returns the entities re-scored.
CREATE OR REPLACE FUNCTION trending.refresh_agenda_listings()
RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
  claim_ids uuid[];
  topic_ids uuid[];
  ids uuid[];
  horizon integer;
  today date;
BEGIN
  -- One writer at a time; the CronJob forbids overlap too.
  PERFORM pg_advisory_xact_lock(hashtext('trending.agenda_listings'));
  SELECT c.trending_horizon_days, trending.today() INTO horizon, today
  FROM public.entity_ranking_config c WHERE c.id;

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

  -- Every claim with a claim listing, and every claim whose listing went: the age credit has no
  -- horizon, and a debate can gain or lose its Claims link at any time. Topics whose rows changed
  -- (added, removed or re-dated) and those inside the horizon: past it a topic lifts nothing.
  SELECT coalesce(array_agg(DISTINCT x.entity_id) FILTER (WHERE x.source <> 'agenda_topics'), '{}'),
         coalesce(array_agg(DISTINCT x.entity_id) FILTER (WHERE x.source = 'agenda_topics'
                                                            AND (x.changed OR x.listed_on BETWEEN today - horizon AND today)), '{}')
    INTO claim_ids, topic_ids
  FROM (
    SELECT c.entity_id, c.source, c.listed_on, true AS changed FROM changed_agenda_listings c
    UNION ALL
    SELECT i.entity_id, i.source, i.listed_on, false FROM incoming_agenda_listings i
  ) x;

  DELETE FROM trending.agenda_listings l
  USING changed_agenda_listings c
  WHERE NOT c.incoming
    AND (l.agenda_id, l.space_id, l.entity_id, l.listed_on, l.source)
      = (c.agenda_id, c.space_id, c.entity_id, c.listed_on, c.source);
  INSERT INTO trending.agenda_listings (agenda_id, space_id, entity_id, listed_on, source)
  SELECT c.agenda_id, c.space_id, c.entity_id, c.listed_on, c.source
  FROM changed_agenda_listings c WHERE c.incoming;

  -- Those claims and the debates on them; claims and debates tagged with those topics; and anything
  -- still carrying a term (a removed tag or Claims link, a config change), by the partial index.
  SELECT coalesce(array_agg(DISTINCT x.id), '{}') INTO ids
  FROM (
    SELECT unnest(claim_ids) AS id
    UNION ALL
    SELECT d.debate_id FROM trending.debates_on(claim_ids) d
    UNION ALL
    SELECT k.entity_id
    FROM trending.claims_and_debates(ARRAY(
      SELECT tg.from_entity_id FROM public.relations tg
      WHERE tg.to_entity_id = ANY(topic_ids)
        AND tg.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid)) k                -- Topics
    UNION ALL
    SELECT s.entity_id FROM public.entity_ranking_scores s
    WHERE s.trending_score <> 0 OR s.age_credit <> 0
  ) x;
  IF cardinality(ids) = 0 THEN
    RETURN 0;
  END IF;
  RETURN public.refresh_entity_ranking_scores(ids);
END;
$$;
--> statement-breakpoint

-- API fields on Entity, so a surprising ordering can be explained without recomputing. The first four
-- are what the stored score used; the last three are live, and for a debate the listing is its claim's.
CREATE OR REPLACE FUNCTION public.entities_trending_score(e public.entities)
RETURNS numeric LANGUAGE sql STABLE AS $$
  SELECT rs.trending_score FROM public.entity_ranking_scores rs WHERE rs.entity_id = e.id;
$$;
--> statement-breakpoint
CREATE OR REPLACE FUNCTION public.entities_trending_age_credit(e public.entities)
RETURNS numeric LANGUAGE sql STABLE AS $$
  SELECT rs.age_credit FROM public.entity_ranking_scores rs WHERE rs.entity_id = e.id;
$$;
--> statement-breakpoint
CREATE OR REPLACE FUNCTION public.entities_trending_source(e public.entities)
RETURNS text LANGUAGE sql STABLE AS $$
  SELECT rs.trending_source FROM public.entity_ranking_scores rs WHERE rs.entity_id = e.id;
$$;
--> statement-breakpoint
CREATE OR REPLACE FUNCTION public.entities_trending_topic_id(e public.entities)
RETURNS uuid LANGUAGE sql STABLE AS $$
  SELECT rs.trending_topic_id FROM public.entity_ranking_scores rs WHERE rs.entity_id = e.id;
$$;
--> statement-breakpoint
CREATE OR REPLACE FUNCTION public.entities_last_listed_on(e public.entities)
RETURNS date LANGUAGE sql STABLE AS $$
  SELECT t.last_listed_on FROM trending.entity_listings(ARRAY[e.id]) t;
$$;
--> statement-breakpoint
CREATE OR REPLACE FUNCTION public.entities_listed_on_agenda_id(e public.entities)
RETURNS uuid LANGUAGE sql STABLE AS $$
  SELECT t.agenda_id FROM trending.entity_listings(ARRAY[e.id]) t;
$$;
--> statement-breakpoint
CREATE OR REPLACE FUNCTION public.entities_trending_topic_listed_on(e public.entities)
RETURNS date LANGUAGE sql STABLE AS $$
  SELECT t.listed_on FROM trending.entity_topic_listings(ARRAY[e.id]) t;
$$;
--> statement-breakpoint

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'gaia_app') AND current_user <> 'gaia_app' THEN
    EXECUTE 'GRANT USAGE ON SCHEMA trending TO gaia_app';
    EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA trending TO gaia_app';
    EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA trending TO gaia_app';
  END IF;
END
$$;
--> statement-breakpoint

-- No sweep here: this transaction holds the ALTERs' locks on entity_ranking_scores and the config,
-- so the first hourly run applies the terms. Bump For you's feed revision (0102), whose trigger an
-- ADD COLUMN does not fire, so its analytics can tell the rankings apart.
UPDATE "entity_ranking_config" SET "updated_at" = now() WHERE "id";
