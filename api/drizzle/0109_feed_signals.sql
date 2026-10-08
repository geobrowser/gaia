-- GEO-3235: behaviour signals from analytics, in tables the feed can read cheaply.
--
-- WHAT. Two serving tables and a run log, written hourly by ranking-indexer's `feed_signals_sync`
-- from three ClickHouse views (analytics migration 107), which read the dashboards' own
-- bot-filtered event set:
--   feed_item_signals_hourly  per Explore item and UTC hour: impressions (and by position band),
--                             opens, votes from Explore, debate plays, watch time, watched fraction.
--   feed_user_seen_daily      per signed-in user (personal space), item and UTC day: how many
--                             times the item was shown to them on Explore, and when last.
--   feed_signal_runs          one row per run: the analytics generation read, its watermark
--                             (`data_through`), the window replaced and what was written.
--
-- WHY HOURLY BUCKETS, NOT ROLLING WINDOWS OR COUNTERS. The consumers want different windows:
-- exposure-corrected engagement wants 7 days, fresh-slot exit wants "impressions since it was
-- new", seen-demotion wants the last 14 days per user. Buckets serve all of them with one sum at
-- read time, and they make a run idempotent: a run REPLACES every bucket from its window start to
-- the watermark, so a rerun writes the same rows, a missed hour is simply inside the next window,
-- and nothing is ever added to a counter twice.
--
-- THE WINDOW. Each run starts at UTC midnight `revise_days` (7) before the new watermark, or at the
-- last successful watermark if that is older (a long outage catches up from where it stopped),
-- but never more than `backfill_days` (30) back; the first run backfills 30 days. Re-reading a
-- week every hour is cheap at Explore's volume (thousands of events a day after bot filtering),
-- and it also absorbs late-arriving events and the crawler rules that classify a session days
-- later (the weekly fleet rules): a session reclassified as a bot drops out of every bucket it
-- touched within the week.
--
-- READS. `feed_user_seen(user, since)` is one range scan of the primary key (user_id, day, ...);
-- `feed_item_signals(items, since)` sums an item's buckets. Both are STABLE SQL functions in this
-- private schema, for the api's internal routes and the ranking jobs; nothing here is GraphQL.
--
-- PRIVACY. The `personalization` schema is not introspected by PostGraphile; every name below is
-- also in HIDDEN_SURFACE (api/src/kg/securitySignals.ts). Per-user rows hold a personal space id,
-- an item id, a day, a count and a time, and are deleted after `user_retention_days` (at most 30)
-- by every run. They are personal data and belong in the erasure runbook (GEO-3225): delete the
-- user's rows here AND their events in ClickHouse, or the next run (which re-reads a week)
-- writes them back.
--
-- IDENTITY. Analytics hands over wallet hashes, never Privy ids:
-- 'sha256:' || hex(sha256('wallet:' || lower(address))). A personal space's address is its owner's
-- Privy embedded wallet, so hashing `spaces.address` the same way places each row on a personal
-- space; the same mapping the test-account exclusion sync uses (GEO-3141). Rows that place on no
-- personal space (a signed-in user who has not created one yet) are counted and dropped.

-- The job's knobs. One row.
CREATE TABLE IF NOT EXISTS "personalization"."feed_signal_config" (
	"id" boolean PRIMARY KEY DEFAULT true CHECK ("id"),
	-- Every run re-reads at least this many days before the watermark.
	"revise_days" integer NOT NULL DEFAULT 7 CHECK ("revise_days" BETWEEN 1 AND 30),
	-- The furthest back any run reads: the first run's backfill, and the cap on catching up.
	"backfill_days" integer NOT NULL DEFAULT 30 CHECK ("backfill_days" BETWEEN 1 AND 90),
	-- Per-user rows older than this are deleted by every run. At most 30 days (GEO-3235).
	"user_retention_days" integer NOT NULL DEFAULT 30 CHECK ("user_retention_days" BETWEEN 1 AND 30),
	"item_retention_days" integer NOT NULL DEFAULT 90 CHECK ("item_retention_days" BETWEEN 7 AND 400),
	"run_retention_days" integer NOT NULL DEFAULT 90 CHECK ("run_retention_days" >= 1),
	-- Freshness: stale when no run has succeeded for this long ...
	"max_run_age" interval NOT NULL DEFAULT '3 hours',
	-- ... or the newest data read is older than this (analytics stopped publishing).
	"max_data_age" interval NOT NULL DEFAULT '6 hours',
	CHECK ("backfill_days" >= "revise_days")
);
--> statement-breakpoint
INSERT INTO "personalization"."feed_signal_config" ("id") VALUES (true) ON CONFLICT DO NOTHING;
--> statement-breakpoint

CREATE TABLE IF NOT EXISTS "personalization"."feed_item_signals_hourly" (
	"item_id" uuid NOT NULL,
	-- Start of the UTC hour.
	"hour" timestamptz NOT NULL,
	-- Explore feed cards shown (at least half in view), and by 1-based position. Impressions with
	-- no recorded position count in `impressions` only.
	"impressions" integer NOT NULL DEFAULT 0,
	"impressions_top3" integer NOT NULL DEFAULT 0,
	"impressions_4_10" integer NOT NULL DEFAULT 0,
	"impressions_11_30" integer NOT NULL DEFAULT 0,
	"impressions_31_plus" integer NOT NULL DEFAULT 0,
	-- Cards opened (side panel or page) from Explore.
	"opens" integer NOT NULL DEFAULT 0,
	-- Successful votes made on the Explore page, credited to what was voted on.
	"votes" integer NOT NULL DEFAULT 0,
	-- Debate playbacks that started this hour, from any page, and those on Explore.
	"plays" integer NOT NULL DEFAULT 0,
	"explore_plays" integer NOT NULL DEFAULT 0,
	-- Active (foreground, playing, in view) milliseconds of those plays.
	"watch_ms" bigint NOT NULL DEFAULT 0,
	"explore_watch_ms" bigint NOT NULL DEFAULT 0,
	-- Plays whose media duration was known; the sum of their watched fractions (media covered over
	-- duration, capped at 1); and how many covered at least 90%. Mean watch-through is
	-- watch_fraction_sum / plays_with_duration.
	"plays_with_duration" integer NOT NULL DEFAULT 0,
	"watch_fraction_sum" double precision NOT NULL DEFAULT 0,
	"completed_plays" integer NOT NULL DEFAULT 0,
	PRIMARY KEY ("item_id", "hour")
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "feed_item_signals_hourly_hour_idx" ON "personalization"."feed_item_signals_hourly" ("hour");
--> statement-breakpoint

CREATE TABLE IF NOT EXISTS "personalization"."feed_user_seen_daily" (
	-- The viewer's personal space id (user_votes.user_id).
	"user_id" uuid NOT NULL,
	-- The UTC day.
	"day" date NOT NULL,
	"item_id" uuid NOT NULL,
	"impressions" integer NOT NULL,
	"last_seen_at" timestamptz NOT NULL,
	PRIMARY KEY ("user_id", "day", "item_id")
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "feed_user_seen_daily_day_idx" ON "personalization"."feed_user_seen_daily" ("day");
--> statement-breakpoint

CREATE TABLE IF NOT EXISTS "personalization"."feed_signal_runs" (
	"id" bigserial PRIMARY KEY,
	"finished_at" timestamptz NOT NULL DEFAULT now(),
	-- succeeded: the window was replaced. unchanged: analytics had published nothing new.
	-- failed: nothing was written; `error` says why.
	"status" text NOT NULL CHECK ("status" IN ('succeeded', 'unchanged', 'failed')),
	-- The analytics input generation read, and its cutoff: the watermark.
	"generation" text,
	"data_through" timestamptz,
	"window_start" timestamptz,
	"window_end" timestamptz,
	"item_rows" integer,
	"user_rows_in" integer,
	"user_rows_placed" integer,
	"user_rows" integer,
	"item_rows_expired" integer,
	"user_rows_expired" integer,
	"error" text
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "feed_signal_runs_status_idx" ON "personalization"."feed_signal_runs" ("status", "finished_at" DESC);
--> statement-breakpoint

-- analytics' identity_hash('wallet', address), from an address. Must match analytics exactly
-- (crates/analytics-ingest/src/identity.rs); feed_signals_sync checks a known pair at startup.
CREATE OR REPLACE FUNCTION personalization.wallet_address_hash(address text)
RETURNS text
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
	SELECT 'sha256:' || encode(sha256(convert_to('wallet:' || lower(btrim(address)), 'UTF8')), 'hex')
$$;
--> statement-breakpoint
COMMENT ON FUNCTION personalization.wallet_address_hash(text) IS E'@omit';
--> statement-breakpoint

-- The window the next run must replace, given the watermark analytics now offers.
-- `unchanged` when that watermark is not newer than the last successful run's: nothing to do.
CREATE OR REPLACE FUNCTION personalization.feed_signal_window(p_data_through timestamptz)
RETURNS TABLE (window_start timestamptz, window_end timestamptz, last_data_through timestamptz, unchanged boolean)
LANGUAGE sql STABLE AS $$
	WITH c AS (SELECT * FROM personalization.feed_signal_config WHERE id),
	last AS (
		SELECT max(r.data_through) AS data_through
		FROM personalization.feed_signal_runs r WHERE r.status = 'succeeded'
	)
	SELECT
		date_trunc('day',
			greatest(p_data_through - make_interval(days => c.backfill_days),
			         least(coalesce(last.data_through, '-infinity'::timestamptz),
			               p_data_through - make_interval(days => c.revise_days))),
			'UTC'),
		p_data_through,
		last.data_through,
		last.data_through IS NOT NULL AND p_data_through <= last.data_through
	FROM c, last
$$;
--> statement-breakpoint
COMMENT ON FUNCTION personalization.feed_signal_window(timestamptz) IS E'@omit';
--> statement-breakpoint

-- Replace every bucket from p_window_start (a UTC midnight) onwards with the rows analytics returned
-- for [p_window_start, p_window_end), drop rows past retention, and log the run, in one transaction.
--
--   p_items  [{item_id, hour, impressions, impressions_top3, impressions_4_10, impressions_11_30,
--              impressions_31_plus, opens, votes, plays, explore_plays, watch_ms, explore_watch_ms,
--              plays_with_duration, watch_fraction_sum, completed_plays}]   (analytics.feed_signal_items)
--   p_users  [{wallet_address_hash, item_id, day, impressions, last_seen_at}]  (analytics.feed_signal_user_items)
--
-- Refuses (and writes nothing) when the window is not day-aligned, when analytics returned no items
-- for a window that already holds some (a broken read must not erase a week), or when user rows came
-- back and none placed on a personal space while the window holds some (the identity mapping
-- broke). p_allow_empty overrides both.
CREATE OR REPLACE FUNCTION personalization.replace_feed_signals(
	p_generation text,
	p_window_start timestamptz,
	p_window_end timestamptz,
	p_items jsonb,
	p_users jsonb,
	p_allow_empty boolean DEFAULT false,
	p_as_of timestamptz DEFAULT now()
)
RETURNS TABLE (run_id bigint, item_rows integer, user_rows_in integer, user_rows_placed integer,
               user_rows integer, item_rows_expired integer, user_rows_expired integer)
LANGUAGE plpgsql AS $$
DECLARE
	cfg personalization.feed_signal_config%ROWTYPE;
	start_day date := (p_window_start AT TIME ZONE 'UTC')::date;
	n_items integer;
	n_users_in integer;
	n_placed integer;
	n_users integer;
	n_items_expired integer;
	n_users_expired integer;
	n_existing integer;
	new_id bigint;
BEGIN
	IF p_window_start IS NULL OR p_window_end IS NULL OR p_window_end <= p_window_start THEN
		RAISE EXCEPTION 'replace_feed_signals: empty window [%, %)', p_window_start, p_window_end;
	END IF;
	IF p_window_start <> date_trunc('day', p_window_start, 'UTC') THEN
		RAISE EXCEPTION 'replace_feed_signals: window_start % is not a UTC midnight', p_window_start;
	END IF;
	IF jsonb_typeof(p_items) IS DISTINCT FROM 'array' OR jsonb_typeof(p_users) IS DISTINCT FROM 'array' THEN
		RAISE EXCEPTION 'replace_feed_signals: p_items and p_users must be JSON arrays';
	END IF;
	SELECT * INTO cfg FROM personalization.feed_signal_config WHERE id;

	-- One writer at a time; the CronJob forbids overlap too.
	PERFORM pg_advisory_xact_lock(hashtext('personalization.feed_signals'));

	IF jsonb_array_length(p_items) = 0 AND NOT p_allow_empty THEN
		SELECT count(*) INTO n_existing FROM personalization.feed_item_signals_hourly h WHERE h.hour >= p_window_start;
		IF n_existing > 0 THEN
			RAISE EXCEPTION 'replace_feed_signals: analytics returned no items for a window holding % rows; refusing to erase them', n_existing;
		END IF;
	END IF;

	DELETE FROM personalization.feed_item_signals_hourly h WHERE h.hour >= p_window_start;
	INSERT INTO personalization.feed_item_signals_hourly
		(item_id, hour, impressions, impressions_top3, impressions_4_10, impressions_11_30, impressions_31_plus,
		 opens, votes, plays, explore_plays, watch_ms, explore_watch_ms, plays_with_duration, watch_fraction_sum,
		 completed_plays)
	SELECT r.item_id, r.hour, r.impressions, r.impressions_top3, r.impressions_4_10, r.impressions_11_30,
	       r.impressions_31_plus, r.opens, r.votes, r.plays, r.explore_plays, r.watch_ms, r.explore_watch_ms,
	       r.plays_with_duration, r.watch_fraction_sum, r.completed_plays
	FROM jsonb_to_recordset(p_items) AS r(
		item_id uuid, hour timestamptz, impressions integer, impressions_top3 integer, impressions_4_10 integer,
		impressions_11_30 integer, impressions_31_plus integer, opens integer, votes integer, plays integer,
		explore_plays integer, watch_ms bigint, explore_watch_ms bigint, plays_with_duration integer,
		watch_fraction_sum double precision, completed_plays integer)
	WHERE r.item_id IS NOT NULL AND r.hour >= p_window_start AND r.hour < p_window_end;
	GET DIAGNOSTICS n_items = ROW_COUNT;

	-- Place each wallet hash on a personal space. Several rows can land on one (user, day, item)
	-- only if one account holds several wallets that are all personal-space addresses; sum them.
	CREATE TEMP TABLE IF NOT EXISTS incoming_feed_seen (
		user_id uuid, day date, item_id uuid, impressions integer, last_seen_at timestamptz
	) ON COMMIT DROP;
	TRUNCATE incoming_feed_seen;
	n_users_in := jsonb_array_length(p_users);
	INSERT INTO incoming_feed_seen
	SELECT s.id, r.day, r.item_id, r.impressions, r.last_seen_at
	FROM jsonb_to_recordset(p_users) AS r(wallet_address_hash text, item_id uuid, day date, impressions integer, last_seen_at timestamptz)
	JOIN public.spaces s
	  ON s.type = 'Personal'
	 AND personalization.wallet_address_hash(s.address) = lower(btrim(r.wallet_address_hash))
	WHERE r.item_id IS NOT NULL AND r.day >= start_day AND r.last_seen_at < p_window_end;
	GET DIAGNOSTICS n_placed = ROW_COUNT;

	IF n_users_in > 0 AND n_placed = 0 AND NOT p_allow_empty THEN
		SELECT count(*) INTO n_existing FROM personalization.feed_user_seen_daily u WHERE u.day >= start_day;
		IF n_existing > 0 THEN
			RAISE EXCEPTION 'replace_feed_signals: none of % user rows is on a personal space while the window holds %; the identity mapping looks broken',
				n_users_in, n_existing;
		END IF;
	END IF;

	DELETE FROM personalization.feed_user_seen_daily u WHERE u.day >= start_day;
	INSERT INTO personalization.feed_user_seen_daily (user_id, day, item_id, impressions, last_seen_at)
	SELECT i.user_id, i.day, i.item_id, sum(i.impressions), max(i.last_seen_at)
	FROM incoming_feed_seen i
	GROUP BY i.user_id, i.day, i.item_id;
	GET DIAGNOSTICS n_users = ROW_COUNT;

	-- Retention, every run.
	DELETE FROM personalization.feed_user_seen_daily u
	WHERE u.day < (p_as_of AT TIME ZONE 'UTC')::date - cfg.user_retention_days;
	GET DIAGNOSTICS n_users_expired = ROW_COUNT;
	DELETE FROM personalization.feed_item_signals_hourly h
	WHERE h.hour < p_as_of - make_interval(days => cfg.item_retention_days);
	GET DIAGNOSTICS n_items_expired = ROW_COUNT;
	DELETE FROM personalization.feed_signal_runs r
	WHERE r.finished_at < p_as_of - make_interval(days => cfg.run_retention_days);

	INSERT INTO personalization.feed_signal_runs
		(finished_at, status, generation, data_through, window_start, window_end, item_rows, user_rows_in,
		 user_rows_placed, user_rows, item_rows_expired, user_rows_expired)
	VALUES (p_as_of, 'succeeded', p_generation, p_window_end, p_window_start, p_window_end, n_items, n_users_in,
	        n_placed, n_users, n_items_expired, n_users_expired)
	RETURNING id INTO new_id;

	RETURN QUERY SELECT new_id, n_items, n_users_in, n_placed, n_users, n_items_expired, n_users_expired;
END;
$$;
--> statement-breakpoint
COMMENT ON FUNCTION personalization.replace_feed_signals(text, timestamptz, timestamptz, jsonb, jsonb, boolean, timestamptz) IS E'@omit';
--> statement-breakpoint

-- A run that wrote no signals: analytics had nothing new, or the run failed before writing.
CREATE OR REPLACE FUNCTION personalization.record_feed_signal_run(
	p_status text,
	p_generation text,
	p_data_through timestamptz,
	p_error text DEFAULT NULL,
	p_as_of timestamptz DEFAULT now()
)
RETURNS bigint
LANGUAGE sql AS $$
	INSERT INTO personalization.feed_signal_runs (finished_at, status, generation, data_through, error)
	VALUES (p_as_of, p_status, p_generation, p_data_through, left(p_error, 2000))
	RETURNING id
$$;
--> statement-breakpoint
COMMENT ON FUNCTION personalization.record_feed_signal_run(text, text, timestamptz, text, timestamptz) IS E'@omit';
--> statement-breakpoint

-- Stale when no run has succeeded (or found nothing new) for max_run_age, or the newest data read
-- is older than max_data_age. The freshness CronJob fails on `stale`, which pages via KubeJobFailed.
CREATE OR REPLACE FUNCTION personalization.feed_signal_freshness(p_as_of timestamptz DEFAULT now())
RETURNS TABLE (last_run_at timestamptz, data_through timestamptz, run_age interval, data_age interval,
               stale boolean, reason text)
LANGUAGE sql STABLE AS $$
	WITH c AS (SELECT * FROM personalization.feed_signal_config WHERE id),
	ok AS (
		SELECT max(r.finished_at) AS last_run_at, max(r.data_through) FILTER (WHERE r.status = 'succeeded') AS data_through
		FROM personalization.feed_signal_runs r WHERE r.status IN ('succeeded', 'unchanged')
	)
	SELECT ok.last_run_at, ok.data_through, p_as_of - ok.last_run_at, p_as_of - ok.data_through,
	       ok.data_through IS NULL OR p_as_of - ok.last_run_at > c.max_run_age OR p_as_of - ok.data_through > c.max_data_age,
	       CASE
	         WHEN ok.data_through IS NULL THEN 'no successful run'
	         WHEN p_as_of - ok.last_run_at > c.max_run_age THEN 'no successful run for ' || (p_as_of - ok.last_run_at)::text
	         WHEN p_as_of - ok.data_through > c.max_data_age THEN 'newest analytics data is ' || (p_as_of - ok.data_through)::text || ' old'
	       END
	FROM c, ok
$$;
--> statement-breakpoint
COMMENT ON FUNCTION personalization.feed_signal_freshness(timestamptz) IS E'@omit';
--> statement-breakpoint

-- What one user was shown on Explore since p_since: each item, how many times, and when last.
-- One range scan of the primary key. Counts are whole UTC days, so an item shown on the day p_since
-- falls in counts that day's impressions only if it was last seen at or after p_since.
CREATE OR REPLACE FUNCTION personalization.feed_user_seen(p_user_id uuid, p_since timestamptz)
RETURNS TABLE (item_id uuid, seen_count bigint, last_seen_at timestamptz)
LANGUAGE sql STABLE AS $$
	SELECT u.item_id, sum(u.impressions)::bigint, max(u.last_seen_at)
	FROM personalization.feed_user_seen_daily u
	WHERE u.user_id = p_user_id AND u.day >= (p_since AT TIME ZONE 'UTC')::date
	GROUP BY u.item_id
	HAVING max(u.last_seen_at) >= p_since
	ORDER BY max(u.last_seen_at) DESC
$$;
--> statement-breakpoint
COMMENT ON FUNCTION personalization.feed_user_seen(uuid, timestamptz) IS E'@omit';
--> statement-breakpoint

-- Each item's totals over [p_since, p_until), for the given items (NULL: every item with a row).
-- `first_impression_at` is the hour of its first Explore impression still within retention.
CREATE OR REPLACE FUNCTION personalization.feed_item_signals(
	p_item_ids uuid[],
	p_since timestamptz,
	p_until timestamptz DEFAULT 'infinity'
)
RETURNS TABLE (item_id uuid, impressions bigint, impressions_top3 bigint, impressions_4_10 bigint,
               impressions_11_30 bigint, impressions_31_plus bigint, opens bigint, votes bigint, plays bigint,
               explore_plays bigint, watch_ms bigint, explore_watch_ms bigint, plays_with_duration bigint,
               watch_fraction_sum double precision, completed_plays bigint, first_impression_at timestamptz)
LANGUAGE sql STABLE AS $$
	SELECT h.item_id, sum(h.impressions), sum(h.impressions_top3), sum(h.impressions_4_10), sum(h.impressions_11_30),
	       sum(h.impressions_31_plus), sum(h.opens), sum(h.votes), sum(h.plays), sum(h.explore_plays),
	       sum(h.watch_ms)::bigint, sum(h.explore_watch_ms)::bigint, sum(h.plays_with_duration), sum(h.watch_fraction_sum),
	       sum(h.completed_plays),
	       (SELECT min(f.hour) FROM personalization.feed_item_signals_hourly f
	         WHERE f.item_id = h.item_id AND f.impressions > 0)
	FROM personalization.feed_item_signals_hourly h
	WHERE (p_item_ids IS NULL OR h.item_id = ANY (p_item_ids))
	  AND h.hour >= p_since AND h.hour < p_until
	GROUP BY h.item_id
$$;
--> statement-breakpoint
COMMENT ON FUNCTION personalization.feed_item_signals(uuid[], timestamptz, timestamptz) IS E'@omit';
--> statement-breakpoint

DO $$
BEGIN
	IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'gaia_app') AND current_user <> 'gaia_app' THEN
		EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA personalization TO gaia_app';
		EXECUTE 'GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA personalization TO gaia_app';
		EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA personalization TO gaia_app';
	END IF;
END
$$;
