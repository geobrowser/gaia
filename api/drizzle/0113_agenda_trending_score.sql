-- GEO-3488: Best ranks the claims on a Debate agenda, and then the debates recorded on them, at the top.
--
-- WHY. Best is mostly recency plus vote volume, so a claim put on today's agenda with no votes misses
-- the top of every ranked list, and a year-old claim we list today is buried by its age. Each debate
-- space now publishes a daily Debate agenda (GEO-3487): the claims we want debated today. Once one
-- is debated, the debate recorded on it should take its place, so the content stays visible as video.
--
-- WHAT A LISTING IS. A Debate agenda (type 79e3757b) in space S, with a Date (52665f3e, a datetime
-- written as midnight New York) in S, has Blocks (beaba5cb) in S to data blocks, whose Collection
-- items (a99f9ce1) in S point at claims. Every relation and the Date must be written in S, the
-- agenda's own space: an agenda copied into another space lists nothing there unless that space
-- also has its own relations. Every published agenda counts, including an earlier agenda for the
-- same space and Date; the newer one is only the freshest. A listing counts once its Date arrives
-- in New York, so tomorrow's agenda published tonight does nothing until midnight. The relations
-- table has no timestamps and nothing re-scores an entity when a relation changes, so the listings
-- are rebuilt in full every hour into trending.agenda_listings (a few hundred rows a day; an edited
-- or deleted agenda is handled by the rebuild for free).
--
-- THE TERMS, both already included in ranking_score, both in score units (one unit is tau, 100000 s,
-- about 28 hours of recency):
--
--   claim   age_credit     = max(0, (midnight of last_listed_on − created_at) / tau)
--           trending_score = boost × day_factor^days × debate_factor^debates_in_window
--   debate  trending_score = boost × day_factor^days        (its claim's days; no age credit)
--
--   days             whole days from the claim's newest arrived listing to today (New York).
--   debates_in_window  debates on the claim (typed Debate, Claims relation) created in the last
--                    trending_debate_window_days (7).
--   A debate earns its claim's boost only if it was created on or after the day of the claim's
--   first listing; one recorded before the claim was ever listed gets nothing. Every debate on
--   the claim after that gets the same boost: the watch term (0111) separates the best of them.
--   With several claims, a debate takes the largest.
--
-- The age credit makes a listed claim rank as if it were created on its newest listing day, and it
-- is permanent: it is read from the listings table on every re-score, including the one each vote
-- triggers, so it survives everything except the agenda being removed. The boost keeps halving with
-- no product cutoff. For bookkeeping it is exactly 0 once `days` passes trending_horizon_days (11),
-- where 15 × 0.5^11 = 0.007, so the hourly sweep set stays bounded by construction.
--
-- NUMBERS. A boost of 15 is worth about 17 days of recency (15 × 100000 s), so a claim listed today
-- with no votes outranks every unlisted claim except a same-day one with heavy voting (participation
-- caps at 19). Halving per day takes it under one day of recency (0.86) by day 5. Each debate in the
-- past week takes a quarter off, less than one day of decay. Two different debate counts, on
-- purpose: this discount counts debates in the past 7 days, while the app's "Debated N×" counts
-- debates since the last listing, from geo-chat. When a debate leaves the 7-day window the claim's
-- boost rises by a third of something already tiny.
--
-- All of it is configuration in entity_ranking_config. Boost 0 turns the boost off; the age credit
-- has no switch of its own because it only moves a claim to the date we listed it.
--
-- FRESHNESS. The terms live in refresh_entity_ranking_scores, so every re-score applies them.
-- trending.refresh_agenda_listings(), run hourly by ranking-indexer's agenda_listings_refresh,
-- rebuilds the listings and re-scores, in the same transaction: claims whose listings changed
-- (added, removed, re-dated), claims listed in the last trending_horizon_days, the debates of all
-- of those, and anything still storing a non-zero trending term (config changes, removed agendas).
--
-- API. Entity gets four computed fields: trendingScore and trendingAgeCredit (stored, what the score
-- used), lastListedOn and listedOnAgendaId (live; for a debate, its claim's). The listings table
-- lives in the private `trending` schema because PostGraphile publishes every public table.

CREATE SCHEMA IF NOT EXISTS "trending";
--> statement-breakpoint

ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "trending_boost" numeric NOT NULL DEFAULT 15;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "trending_day_factor" numeric NOT NULL DEFAULT 0.5;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "trending_debate_factor" numeric NOT NULL DEFAULT 0.75;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "trending_debate_window_days" integer NOT NULL DEFAULT 7;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "trending_horizon_days" integer NOT NULL DEFAULT 11;
--> statement-breakpoint
ALTER TABLE "entity_ranking_config"
  ADD COLUMN IF NOT EXISTS "trending_time_zone" text NOT NULL DEFAULT 'America/New_York';
--> statement-breakpoint
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'entity_ranking_config_trending_terms_valid') THEN
    ALTER TABLE "entity_ranking_config" ADD CONSTRAINT "entity_ranking_config_trending_terms_valid"
      CHECK ("trending_boost" >= 0 AND "trending_day_factor" > 0 AND "trending_day_factor" <= 1
             AND "trending_debate_factor" > 0 AND "trending_debate_factor" <= 1
             AND "trending_debate_window_days" > 0 AND "trending_horizon_days" >= 0);
  END IF;
END
$$;
--> statement-breakpoint

-- Stored for explainability, like the other terms, and already included in ranking_score. 0 for
-- everything not listed. Constant defaults, so adding them rewrites no rows.
ALTER TABLE "entity_ranking_scores"
  ADD COLUMN IF NOT EXISTS "age_credit" numeric NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE "entity_ranking_scores"
  ADD COLUMN IF NOT EXISTS "trending_score" numeric NOT NULL DEFAULT 0;
--> statement-breakpoint

-- Every listing of a claim on a published agenda, future-dated ones included (readers skip those
-- until their Date arrives). Rebuilt in full by trending.refresh_agenda_listings().
CREATE TABLE IF NOT EXISTS "trending"."agenda_listings" (
  "claim_id" uuid NOT NULL,
  "agenda_id" uuid NOT NULL,
  "space_id" uuid NOT NULL,
  "listed_on" date NOT NULL,
  PRIMARY KEY ("claim_id", "agenda_id", "space_id")
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "agenda_listings_listed_on_idx" ON "trending"."agenda_listings" ("listed_on");
--> statement-breakpoint

-- A Date value's instant. datetime_utc is what the indexer writes; the text column is the fallback,
-- parsed defensively so one malformed value cannot fail the hourly rebuild.
CREATE OR REPLACE FUNCTION trending.value_instant(datetime_utc timestamptz, datetime text)
RETURNS timestamptz
LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN
  IF datetime_utc IS NOT NULL THEN
    RETURN datetime_utc;
  END IF;
  IF datetime IS NULL OR datetime !~ '^\d{4}-\d{2}-\d{2}' THEN
    RETURN NULL;
  END IF;
  RETURN datetime::timestamptz;
EXCEPTION WHEN others THEN
  RETURN NULL;
END;
$$;
--> statement-breakpoint

-- The listings as the graph has them now.
CREATE OR REPLACE FUNCTION trending.current_agenda_listings()
RETURNS TABLE (claim_id uuid, agenda_id uuid, space_id uuid, listed_on date)
LANGUAGE sql STABLE AS $$
  WITH cfg AS (
    SELECT c.trending_time_zone AS tz FROM public.entity_ranking_config c WHERE c.id
  ),
  agendas AS (
    -- One Date per agenda and space; if an edit ever leaves two, the later one.
    SELECT ty.from_entity_id AS agenda_id, ty.space_id,
           max((trending.value_instant(v.datetime_utc, v.datetime) AT TIME ZONE cfg.tz)::date) AS listed_on
    FROM public.relations ty
    CROSS JOIN cfg
    JOIN public.values v
      ON v.entity_id = ty.from_entity_id AND v.space_id = ty.space_id
     AND v.property_id = '52665f3e-fb7d-48d5-8b6b-abb21b0d36db'::uuid               -- Date
    WHERE ty.to_entity_id = '79e3757b-2630-492c-9d3b-d73a78d45893'::uuid           -- Debate agenda
      AND ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid                -- Types
    GROUP BY ty.from_entity_id, ty.space_id
  )
  SELECT DISTINCT item.to_entity_id, a.agenda_id, a.space_id, a.listed_on
  FROM agendas a
  JOIN public.relations blk
    ON blk.from_entity_id = a.agenda_id AND blk.space_id = a.space_id
   AND blk.type_id = 'beaba5cb-a677-41a8-b353-77030613fc70'::uuid                   -- Blocks
  JOIN public.relations item
    ON item.from_entity_id = blk.to_entity_id AND item.space_id = a.space_id
   AND item.type_id = 'a99f9ce1-2ffa-4dac-8c61-f6310d46064a'::uuid                  -- Collection item
  WHERE a.listed_on IS NOT NULL
    AND EXISTS (SELECT 1 FROM public.relations ct
                WHERE ct.from_entity_id = item.to_entity_id
                  AND ct.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid      -- Types
                  AND ct.to_entity_id = '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid) -- Claim
$$;
--> statement-breakpoint

-- Per claim, its arrived listings: the newest (and on which agenda) and the first.
CREATE OR REPLACE FUNCTION trending.claim_listings(claim_ids uuid[])
RETURNS TABLE (claim_id uuid, last_listed_on date, first_listed_on date, agenda_id uuid, space_id uuid)
LANGUAGE sql STABLE AS $$
  SELECT l.claim_id, max(l.listed_on), min(l.listed_on),
         (array_agg(l.agenda_id ORDER BY l.listed_on DESC, l.agenda_id))[1],
         (array_agg(l.space_id ORDER BY l.listed_on DESC, l.agenda_id))[1]
  FROM trending.agenda_listings l
  CROSS JOIN public.entity_ranking_config c
  WHERE c.id AND l.claim_id = ANY(claim_ids)
    AND l.listed_on <= (now() AT TIME ZONE c.trending_time_zone)::date
  GROUP BY l.claim_id
$$;
--> statement-breakpoint

-- The two terms for those of `entity_ids` that are listed claims or debates on one. One row per
-- such entity; nothing for anything else.
CREATE OR REPLACE FUNCTION trending.entity_trending_terms(entity_ids uuid[])
RETURNS TABLE (entity_id uuid, age_credit numeric, trending_score numeric, last_listed_on date,
               agenda_id uuid, days integer, debates_in_window integer)
LANGUAGE sql STABLE AS $$
  WITH cfg AS (
    SELECT c.*, (now() AT TIME ZONE c.trending_time_zone)::date AS today
    FROM public.entity_ranking_config c WHERE c.id
  ),
  target AS (
    SELECT e.id, CASE WHEN e.created_at ~ '^[0-9]+$' THEN e.created_at::bigint END AS created_epoch
    FROM public.entities e
    WHERE e.id = ANY(entity_ids)
  ),
  claims AS (
    SELECT t.id, t.created_epoch FROM target t
    WHERE EXISTS (SELECT 1 FROM public.relations ty
                  WHERE ty.from_entity_id = t.id
                    AND ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid      -- Types
                    AND ty.to_entity_id = '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid) -- Claim
  ),
  debate_claims AS (
    SELECT DISTINCT t.id, t.created_epoch, cl.to_entity_id AS claim_id
    FROM target t
    JOIN public.relations cl
      ON cl.from_entity_id = t.id AND cl.type_id = 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid -- Claims
    WHERE EXISTS (SELECT 1 FROM public.relations ty
                  WHERE ty.from_entity_id = t.id
                    AND ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid      -- Types
                    AND ty.to_entity_id = 'fd51f935-2063-4617-be39-7b672b23364c'::uuid) -- Debate
  ),
  listed AS (
    SELECT * FROM trending.claim_listings(ARRAY(SELECT id FROM claims UNION SELECT claim_id FROM debate_claims))
  ),
  claim_terms AS (
    SELECT c.id, l.last_listed_on, l.agenda_id, (cfg.today - l.last_listed_on) AS days,
           GREATEST(0, (extract(epoch FROM l.last_listed_on::timestamp AT TIME ZONE cfg.trending_time_zone)::numeric
                        - COALESCE(c.created_epoch, 0)) / GREATEST(cfg.tau_seconds, 1)) AS age_credit,
           (SELECT count(DISTINCT d.from_entity_id)::integer
            FROM public.relations d
            JOIN public.entities de ON de.id = d.from_entity_id
            WHERE d.to_entity_id = c.id
              AND d.type_id = 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid               -- Claims
              AND de.created_at ~ '^[0-9]+$'
              AND de.created_at::bigint >= extract(epoch FROM now()) - cfg.trending_debate_window_days * 86400
              AND EXISTS (SELECT 1 FROM public.relations ty
                          WHERE ty.from_entity_id = d.from_entity_id
                            AND ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid
                            AND ty.to_entity_id = 'fd51f935-2063-4617-be39-7b672b23364c'::uuid)) AS debates
    FROM claims c
    JOIN listed l ON l.claim_id = c.id
    CROSS JOIN cfg
  ),
  debate_terms AS (
    -- Only claims listed on or before the day the debate was created; the largest boost wins.
    SELECT DISTINCT ON (dc.id) dc.id, l.last_listed_on, l.agenda_id, (cfg.today - l.last_listed_on) AS days
    FROM debate_claims dc
    JOIN listed l ON l.claim_id = dc.claim_id
    CROSS JOIN cfg
    WHERE dc.created_epoch IS NOT NULL
      AND (to_timestamp(dc.created_epoch) AT TIME ZONE cfg.trending_time_zone)::date >= l.first_listed_on
    ORDER BY dc.id, l.last_listed_on DESC, l.agenda_id
  )
  SELECT ct.id, ct.age_credit,
         CASE WHEN ct.days > cfg.trending_horizon_days THEN 0
              ELSE cfg.trending_boost * power(cfg.trending_day_factor, ct.days)
                                      * power(cfg.trending_debate_factor, ct.debates) END,
         ct.last_listed_on, ct.agenda_id, ct.days, ct.debates
  FROM claim_terms ct CROSS JOIN cfg
  UNION ALL
  SELECT dt.id, 0,
         CASE WHEN dt.days > cfg.trending_horizon_days THEN 0
              ELSE cfg.trending_boost * power(cfg.trending_day_factor, dt.days) END,
         dt.last_listed_on, dt.agenda_id, dt.days, NULL
  FROM debate_terms dt CROSS JOIN cfg
$$;
--> statement-breakpoint

-- The score choke point as 0111 left it, plus the age credit and the trending term. Everything else
-- is unchanged.
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
  -- GEO-3488: rows only for listed claims and debates on them; everything else gets 0 below.
  trend AS (
    SELECT ett.entity_id AS id, ett.age_credit, ett.trending_score
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
     watch_score, age_credit, trending_score, updated_at)
  SELECT c.id, c.quality_score, c.intrinsic_score, c.participation_score, c.comment_score,
         -- Not a topic debate: exactly 0098's expression, plus the watch term (0 if not a debate),
         -- plus the age credit and trending term (0 unless listed or a debate on a listed claim).
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
         c.type_weight, COALESCE(c.topic_score, 0), c.watch_score, c.age_credit, c.trending_score, now()
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

-- Rebuilds trending.agenda_listings from the graph and re-scores every entity whose terms can have
-- moved. One call is one transaction: readers keep the previous listings and scores until it
-- commits. Run hourly by ranking-indexer's agenda_listings_refresh. Returns the entities re-scored.
CREATE OR REPLACE FUNCTION trending.refresh_agenda_listings()
RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
  claim_ids uuid[];
  ids uuid[];
  horizon integer;
  today date;
BEGIN
  -- One writer at a time; the CronJob forbids overlap too.
  PERFORM pg_advisory_xact_lock(hashtext('trending.agenda_listings'));
  SELECT c.trending_horizon_days, (now() AT TIME ZONE c.trending_time_zone)::date
    INTO horizon, today
  FROM public.entity_ranking_config c WHERE c.id;

  CREATE TEMP TABLE IF NOT EXISTS incoming_agenda_listings (
    claim_id uuid, agenda_id uuid, space_id uuid, listed_on date
  ) ON COMMIT DROP;
  TRUNCATE incoming_agenda_listings;
  INSERT INTO incoming_agenda_listings SELECT * FROM trending.current_agenda_listings();

  -- Claims whose listings changed (added, removed or re-dated), and claims inside the horizon.
  SELECT coalesce(array_agg(DISTINCT x.claim_id), '{}') INTO claim_ids
  FROM (
    (SELECT l.claim_id, l.agenda_id, l.space_id, l.listed_on FROM trending.agenda_listings l
     EXCEPT SELECT * FROM incoming_agenda_listings)
    UNION ALL
    (SELECT * FROM incoming_agenda_listings
     EXCEPT SELECT l.claim_id, l.agenda_id, l.space_id, l.listed_on FROM trending.agenda_listings l)
    UNION ALL
    SELECT i.* FROM incoming_agenda_listings i
    WHERE i.listed_on BETWEEN today - horizon AND today
  ) x;

  DELETE FROM trending.agenda_listings;
  INSERT INTO trending.agenda_listings (claim_id, agenda_id, space_id, listed_on)
  SELECT * FROM incoming_agenda_listings;

  -- Those claims, the debates on them, and anything still carrying a trending term.
  SELECT coalesce(array_agg(DISTINCT x.id), '{}') INTO ids
  FROM (
    SELECT unnest(claim_ids) AS id
    UNION ALL
    SELECT d.from_entity_id
    FROM public.relations d
    WHERE d.to_entity_id = ANY(claim_ids)
      AND d.type_id = 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid                   -- Claims
      AND EXISTS (SELECT 1 FROM public.relations ty
                  WHERE ty.from_entity_id = d.from_entity_id
                    AND ty.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid
                    AND ty.to_entity_id = 'fd51f935-2063-4617-be39-7b672b23364c'::uuid)
    UNION ALL
    SELECT s.entity_id FROM public.entity_ranking_scores s WHERE s.trending_score <> 0
  ) x;
  IF cardinality(ids) = 0 THEN
    RETURN 0;
  END IF;
  RETURN public.refresh_entity_ranking_scores(ids);
END;
$$;
--> statement-breakpoint

-- API fields on Entity, so a surprising ordering can be explained without recomputing. The first two
-- are what the stored score used; the last two are live, and for a debate they are its claim's.
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
CREATE OR REPLACE FUNCTION public.entities_last_listed_on(e public.entities)
RETURNS date LANGUAGE sql STABLE AS $$
  SELECT t.last_listed_on FROM trending.entity_trending_terms(ARRAY[e.id]) t;
$$;
--> statement-breakpoint
CREATE OR REPLACE FUNCTION public.entities_listed_on_agenda_id(e public.entities)
RETURNS uuid LANGUAGE sql STABLE AS $$
  SELECT t.agenda_id FROM trending.entity_trending_terms(ARRAY[e.id]) t;
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

-- Apply the terms to whatever is already listed.
SELECT trending.refresh_agenda_listings();
