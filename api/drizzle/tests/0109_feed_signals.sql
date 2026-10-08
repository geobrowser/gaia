-- Assertions for 0109: feed signals from analytics (GEO-3235).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
--   1. The window: a first run backfills 30 days; later runs re-read 7 days; an outage catches up
--      from the last watermark, capped at 30 days; a watermark that has not moved is "unchanged".
--   2. A run replaces its window: a rerun writes the same rows (no double counting), a later run
--      that covers a missed hour adds it and leaves earlier buckets equal, and buckets before the
--      window are untouched.
--   3. Per-user rows are placed on personal spaces by wallet hash; a DAO space's address and an
--      unknown hash are dropped and counted.
--   4. Retention: per-user rows past 30 days and item rows past 90 days are deleted by the run.
--   5. Guards: a window that is not a UTC midnight, an empty item read over a window that holds
--      rows, and user rows that place nowhere while rows exist are refused and write nothing.
--   6. Reads: a user's seen set since a time, and an item's totals over a window.
--   7. Freshness: stale with no run, fresh after one, stale when analytics stops publishing or
--      runs stop; failed runs do not count.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

TRUNCATE spaces, personalization.feed_item_signals_hourly, personalization.feed_user_seen_daily,
         personalization.feed_signal_runs CASCADE;
UPDATE personalization.feed_signal_config
   SET revise_days = 7, backfill_days = 30, user_retention_days = 30, item_retention_days = 90,
       run_retention_days = 90, max_run_age = '3 hours', max_data_age = '6 hours'
 WHERE id;

-- The hash analytics computes for wallet 0x929e...3064 (crates/analytics-ingest/src/identity.rs).
SELECT assert(personalization.wallet_address_hash(' 0x929e5195f039E0becB79B039339077FA17183064 ')
              = 'sha256:645c48b3494f9f1fba91b84d2c1fa55d23cfa134be3e1e488803286c13bff73b',
              '0: the wallet hash matches analytics'' identity_hash');

-- Alice and Bob have personal spaces; a DAO space; Carol signed in but has no space.
CREATE TEMP TABLE ids AS SELECT * FROM (VALUES
  ('alice', 'f0000000-0000-4000-8000-00000000000a'::uuid, '0xAAAA00000000000000000000000000000000aaaa'),
  ('bob',   'f0000000-0000-4000-8000-00000000000b'::uuid, '0xbbbb00000000000000000000000000000000bbbb'),
  ('dao',   'f0000000-0000-4000-8000-0000000000d0'::uuid, '0xdddd00000000000000000000000000000000dddd'),
  ('carol', NULL::uuid,                                   '0xcccc00000000000000000000000000000000cccc'),
  ('i1',    'f0000000-0000-4000-8000-000000000101'::uuid, NULL),
  ('i2',    'f0000000-0000-4000-8000-000000000102'::uuid, NULL)
) AS v(k, id, address);
CREATE OR REPLACE FUNCTION pg_temp.i(key text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM ids WHERE k = key $$;
CREATE OR REPLACE FUNCTION pg_temp.h(key text) RETURNS text LANGUAGE sql AS
  $$ SELECT personalization.wallet_address_hash(address) FROM ids WHERE k = key $$;
INSERT INTO spaces (id, type, address)
SELECT id, (CASE k WHEN 'dao' THEN 'DAO' ELSE 'Personal' END)::"spaceTypes", address FROM ids WHERE k IN ('alice', 'bob', 'dao');

-- An item bucket as analytics returns it (32-hex id, ISO hour).
CREATE OR REPLACE FUNCTION pg_temp.item(key text, hour text, impressions integer, votes integer DEFAULT 0, plays integer DEFAULT 0)
RETURNS jsonb LANGUAGE sql AS $$
  SELECT jsonb_build_object('item_id', replace(pg_temp.i(key)::text, '-', ''), 'hour', hour,
    'impressions', impressions, 'impressions_top3', impressions, 'impressions_4_10', 0, 'impressions_11_30', 0,
    'impressions_31_plus', 0, 'opens', 0, 'votes', votes, 'plays', plays, 'explore_plays', plays,
    'watch_ms', plays * 30000, 'explore_watch_ms', plays * 30000, 'plays_with_duration', plays,
    'watch_fraction_sum', plays * 0.5, 'completed_plays', 0)
$$;
CREATE OR REPLACE FUNCTION pg_temp.seen(who text, key text, day text, impressions integer, last_seen text)
RETURNS jsonb LANGUAGE sql AS $$
  SELECT jsonb_build_object('wallet_address_hash', coalesce(pg_temp.h(who), 'sha256:' || repeat('0', 64)),
    'item_id', replace(pg_temp.i(key)::text, '-', ''), 'day', day, 'impressions', impressions, 'last_seen_at', last_seen)
$$;
CREATE OR REPLACE FUNCTION pg_temp.total(key text) RETURNS bigint LANGUAGE sql AS
  $$ SELECT coalesce(sum(impressions), 0) FROM personalization.feed_item_signals_hourly WHERE item_id = pg_temp.i(key) $$;

-- 1. The window. "Now" is 2026-10-06 12:30 UTC; analytics' watermark is 12:00.
DO $$
DECLARE w record;
BEGIN
  SELECT * INTO w FROM personalization.feed_signal_window('2026-10-06T12:00:00Z');
  PERFORM assert(w.window_start = '2026-09-06T00:00:00Z' AND w.last_data_through IS NULL AND NOT w.unchanged,
                 '1: the first run backfills 30 days, from a UTC midnight');

  INSERT INTO personalization.feed_signal_runs (finished_at, status, data_through)
  VALUES ('2026-10-06T11:10:00Z', 'succeeded', '2026-10-06T11:00:00Z');
  SELECT * INTO w FROM personalization.feed_signal_window('2026-10-06T12:00:00Z');
  PERFORM assert(w.window_start = '2026-09-29T00:00:00Z' AND NOT w.unchanged, '1: a later run re-reads 7 days');
  SELECT * INTO w FROM personalization.feed_signal_window('2026-10-06T11:00:00Z');
  PERFORM assert(w.unchanged, '1: a watermark that has not moved is unchanged');

  TRUNCATE personalization.feed_signal_runs;
  INSERT INTO personalization.feed_signal_runs (finished_at, status, data_through)
  VALUES ('2026-09-26T05:00:00Z', 'succeeded', '2026-09-26T04:00:00Z'),
         ('2026-10-06T11:00:00Z', 'failed', '2026-10-06T11:00:00Z');
  SELECT * INTO w FROM personalization.feed_signal_window('2026-10-06T12:00:00Z');
  PERFORM assert(w.window_start = '2026-09-26T00:00:00Z' AND w.last_data_through = '2026-09-26T04:00:00Z',
                 '1: after a 10-day outage the run catches up from the last success (failed runs do not count)');

  TRUNCATE personalization.feed_signal_runs;
  INSERT INTO personalization.feed_signal_runs (finished_at, status, data_through)
  VALUES ('2026-08-01T00:00:00Z', 'succeeded', '2026-08-01T00:00:00Z');
  SELECT * INTO w FROM personalization.feed_signal_window('2026-10-06T12:00:00Z');
  PERFORM assert(w.window_start = '2026-09-06T00:00:00Z', '1: ...but never more than 30 days back');
  TRUNCATE personalization.feed_signal_runs;
END $$;

-- 2 and 3. Two runs over the same hours, then one that catches up a missed hour.
CREATE TEMP TABLE first_run AS SELECT * FROM personalization.feed_item_signals_hourly LIMIT 0;
DO $$
DECLARE r record; before_window bigint;
BEGIN
  -- An old bucket, before any window here: must survive every run.
  INSERT INTO personalization.feed_item_signals_hourly (item_id, hour, impressions)
  VALUES (pg_temp.i('i1'), '2026-09-20T08:00:00Z', 7);

  SELECT * INTO r FROM personalization.replace_feed_signals('g1', '2026-09-29T00:00:00Z', '2026-10-06T11:00:00Z',
    jsonb_build_array(pg_temp.item('i1', '2026-10-06T09:00:00Z', 3, 1),
                      pg_temp.item('i1', '2026-10-06T10:00:00Z', 2),
                      pg_temp.item('i2', '2026-10-06T10:00:00Z', 4, 0, 2),
                      pg_temp.item('i2', '2026-10-06T11:00:00Z', 9)),            -- at window_end: outside
    jsonb_build_array(pg_temp.seen('alice', 'i1', '2026-10-06', 3, '2026-10-06T10:05:00Z'),
                      pg_temp.seen('bob',   'i1', '2026-10-06', 1, '2026-10-06T09:30:00Z'),
                      pg_temp.seen('dao',   'i1', '2026-10-06', 1, '2026-10-06T09:30:00Z'),
                      pg_temp.seen('carol', 'i2', '2026-10-06', 2, '2026-10-06T10:00:00Z')),
    false, '2026-10-06T11:10:00Z');
  PERFORM assert(r.item_rows = 3 AND pg_temp.total('i1') = 3 + 2 + 7 AND pg_temp.total('i2') = 4,
                 '2: the window''s buckets are written, a bucket at window_end is not, an older bucket survives');
  PERFORM assert(r.user_rows_in = 4 AND r.user_rows_placed = 2 AND r.user_rows = 2,
                 '3: rows land on personal spaces only (not a DAO, not an account without a space)');
  PERFORM assert((SELECT impressions FROM personalization.feed_user_seen_daily
                  WHERE user_id = pg_temp.i('alice') AND item_id = pg_temp.i('i1')) = 3,
                 '3: placed by the hash of the space address, whatever its case');
  PERFORM assert((SELECT status = 'succeeded' AND generation = 'g1' AND data_through = '2026-10-06T11:00:00Z'
                    AND window_start = '2026-09-29T00:00:00Z'
                  FROM personalization.feed_signal_runs WHERE id = r.run_id), '2: the run is logged with its watermark');
  INSERT INTO first_run SELECT * FROM personalization.feed_item_signals_hourly;

  -- The same run again: identical rows, nothing doubled.
  SELECT * INTO r FROM personalization.replace_feed_signals('g1', '2026-09-29T00:00:00Z', '2026-10-06T11:00:00Z',
    jsonb_build_array(pg_temp.item('i1', '2026-10-06T09:00:00Z', 3, 1),
                      pg_temp.item('i1', '2026-10-06T10:00:00Z', 2),
                      pg_temp.item('i2', '2026-10-06T10:00:00Z', 4, 0, 2)),
    jsonb_build_array(pg_temp.seen('alice', 'i1', '2026-10-06', 3, '2026-10-06T10:05:00Z'),
                      pg_temp.seen('bob',   'i1', '2026-10-06', 1, '2026-10-06T09:30:00Z')),
    false, '2026-10-06T11:20:00Z');
  PERFORM assert(NOT EXISTS (SELECT * FROM personalization.feed_item_signals_hourly
                             EXCEPT SELECT * FROM first_run)
                 AND NOT EXISTS (SELECT * FROM first_run EXCEPT SELECT * FROM personalization.feed_item_signals_hourly),
                 '2: a rerun of the same window writes exactly the same rows');
  PERFORM assert((SELECT sum(impressions) FROM personalization.feed_user_seen_daily) = 4, '2: ...and the same per-user rows');

  -- The 11:00 run was missed. The 13:00 run's window covers 11:00 and 12:00; earlier hours are
  -- read again and come back the same, the new ones are added.
  SELECT * INTO r FROM personalization.replace_feed_signals('g3', '2026-09-29T00:00:00Z', '2026-10-06T13:00:00Z',
    jsonb_build_array(pg_temp.item('i1', '2026-10-06T09:00:00Z', 3, 1),
                      pg_temp.item('i1', '2026-10-06T10:00:00Z', 2),
                      pg_temp.item('i2', '2026-10-06T10:00:00Z', 4, 0, 2),
                      pg_temp.item('i2', '2026-10-06T11:00:00Z', 9),
                      pg_temp.item('i1', '2026-10-06T12:00:00Z', 1)),
    jsonb_build_array(pg_temp.seen('alice', 'i1', '2026-10-06', 4, '2026-10-06T12:10:00Z'),
                      pg_temp.seen('bob',   'i1', '2026-10-06', 1, '2026-10-06T09:30:00Z')),
    false, '2026-10-06T13:10:00Z');
  PERFORM assert(pg_temp.total('i1') = 3 + 2 + 1 + 7 AND pg_temp.total('i2') = 4 + 9,
                 '2: a missed hour is caught up by the next window, and nothing is counted twice');
  PERFORM assert((SELECT impressions = 4 AND last_seen_at = '2026-10-06T12:10:00Z' FROM personalization.feed_user_seen_daily
                  WHERE user_id = pg_temp.i('alice') AND item_id = pg_temp.i('i1')), '2: a day''s per-user row is replaced, not added to');
END $$;

-- 4. Retention. Now is 2026-10-06; rows on 2026-09-05 (31 days) go, 2026-09-06 (30 days) stay.
DO $$
DECLARE r record;
BEGIN
  INSERT INTO personalization.feed_user_seen_daily (user_id, day, item_id, impressions, last_seen_at) VALUES
    (pg_temp.i('alice'), '2026-09-05', pg_temp.i('i2'), 1, '2026-09-05T10:00:00Z'),
    (pg_temp.i('alice'), '2026-09-06', pg_temp.i('i2'), 1, '2026-09-06T10:00:00Z');
  INSERT INTO personalization.feed_item_signals_hourly (item_id, hour, impressions) VALUES
    (pg_temp.i('i2'), '2026-07-07T13:00:00Z', 5),   -- 91 days old
    (pg_temp.i('i2'), '2026-07-09T13:00:00Z', 6);   -- 89 days old
  SELECT * INTO r FROM personalization.replace_feed_signals('g4', '2026-09-29T00:00:00Z', '2026-10-06T13:00:00Z',
    jsonb_build_array(pg_temp.item('i1', '2026-10-06T12:00:00Z', 1)),
    jsonb_build_array(pg_temp.seen('alice', 'i1', '2026-10-06', 4, '2026-10-06T12:10:00Z')),
    false, '2026-10-06T13:30:00Z');
  PERFORM assert(r.user_rows_expired = 1 AND r.item_rows_expired = 1, '4: one per-user row and one item row expired');
  PERFORM assert(NOT EXISTS (SELECT 1 FROM personalization.feed_user_seen_daily WHERE day < '2026-09-06')
                 AND EXISTS (SELECT 1 FROM personalization.feed_user_seen_daily WHERE day = '2026-09-06'),
                 '4: per-user rows are kept 30 days and no longer');
  PERFORM assert(NOT EXISTS (SELECT 1 FROM personalization.feed_item_signals_hourly WHERE hour = '2026-07-07T13:00:00Z')
                 AND EXISTS (SELECT 1 FROM personalization.feed_item_signals_hourly WHERE hour = '2026-07-09T13:00:00Z'),
                 '4: item rows are kept 90 days');
END $$;

-- 5. Guards. Each refusal leaves the tables as they were.
CREATE TEMP TABLE guard_before AS
SELECT (SELECT count(*) FROM personalization.feed_item_signals_hourly) AS items,
       (SELECT count(*) FROM personalization.feed_user_seen_daily) AS users,
       (SELECT count(*) FROM personalization.feed_signal_runs) AS runs;
DO $$
DECLARE refused boolean; r record;
BEGIN
  refused := false;
  BEGIN
    PERFORM * FROM personalization.replace_feed_signals('g', '2026-09-29T05:00:00Z', '2026-10-06T13:00:00Z', '[]', '[]');
  EXCEPTION WHEN raise_exception THEN refused := true;
  END;
  PERFORM assert(refused, '5: a window that does not start at a UTC midnight is refused');

  refused := false;
  BEGIN
    PERFORM * FROM personalization.replace_feed_signals('g', '2026-09-29T00:00:00Z', '2026-10-06T13:00:00Z', '[]',
      jsonb_build_array(pg_temp.seen('alice', 'i1', '2026-10-06', 4, '2026-10-06T12:10:00Z')));
  EXCEPTION WHEN raise_exception THEN refused := true;
  END;
  PERFORM assert(refused, '5: an empty item read over a window holding rows is refused');

  refused := false;
  BEGIN
    PERFORM * FROM personalization.replace_feed_signals('g', '2026-09-29T00:00:00Z', '2026-10-06T13:00:00Z',
      jsonb_build_array(pg_temp.item('i1', '2026-10-06T12:00:00Z', 1)),
      jsonb_build_array(pg_temp.seen('carol', 'i1', '2026-10-06', 4, '2026-10-06T12:10:00Z')));
  EXCEPTION WHEN raise_exception THEN refused := true;
  END;
  PERFORM assert(refused, '5: user rows that place on no personal space are refused while rows exist');

  PERFORM assert((SELECT items = (SELECT count(*) FROM personalization.feed_item_signals_hourly)
                     AND users = (SELECT count(*) FROM personalization.feed_user_seen_daily)
                     AND runs = (SELECT count(*) FROM personalization.feed_signal_runs) FROM guard_before),
                 '5: ...and every refusal wrote nothing');

  SELECT * INTO r FROM personalization.replace_feed_signals('g5', '2026-10-06T00:00:00Z', '2026-10-06T13:00:00Z', '[]', '[]',
    true, '2026-10-06T13:40:00Z');
  PERFORM assert(r.item_rows = 0 AND pg_temp.total('i1') = 7 AND pg_temp.total('i2') = 6,
                 '5: p_allow_empty clears the window (from its start) and nothing before it');
END $$;

-- 6. Reads.
DO $$
DECLARE s record; t record;
BEGIN
  PERFORM personalization.replace_feed_signals('g6', '2026-10-05T00:00:00Z', '2026-10-06T13:00:00Z',
    jsonb_build_array(pg_temp.item('i1', '2026-10-05T09:00:00Z', 2), pg_temp.item('i1', '2026-10-06T12:00:00Z', 1, 1, 3),
                      pg_temp.item('i2', '2026-10-06T12:00:00Z', 5)),
    jsonb_build_array(pg_temp.seen('alice', 'i1', '2026-10-05', 2, '2026-10-05T09:10:00Z'),
                      pg_temp.seen('alice', 'i1', '2026-10-06', 1, '2026-10-06T12:10:00Z'),
                      pg_temp.seen('alice', 'i2', '2026-10-06', 5, '2026-10-06T12:20:00Z'),
                      pg_temp.seen('bob',   'i2', '2026-10-06', 1, '2026-10-06T12:20:00Z')),
    false, '2026-10-06T13:50:00Z');
  SELECT * INTO s FROM personalization.feed_user_seen(pg_temp.i('alice'), '2026-10-05T00:00:00Z') WHERE item_id = pg_temp.i('i1');
  PERFORM assert(s.seen_count = 3 AND s.last_seen_at = '2026-10-06T12:10:00Z', '6: a user''s seen count and last seen, across days');
  PERFORM assert((SELECT count(*) FROM personalization.feed_user_seen(pg_temp.i('alice'), '2026-10-06T12:15:00Z')) = 1,
                 '6: items last seen before p_since are not in the set');
  PERFORM assert((SELECT count(*) FROM personalization.feed_user_seen(pg_temp.i('bob'), '2026-10-01T00:00:00Z')) = 1,
                 '6: each user sees only their own rows');
  SELECT * INTO t FROM personalization.feed_item_signals(ARRAY[pg_temp.i('i1')], '2026-10-05T00:00:00Z');
  PERFORM assert(t.impressions = 3 AND t.votes = 1 AND t.plays = 3 AND t.watch_ms = 90000 AND t.watch_fraction_sum = 1.5
                 AND t.first_impression_at = '2026-09-20T08:00:00Z', '6: an item''s totals over a window, and its first impression within retention');
  PERFORM assert((SELECT count(*) FROM personalization.feed_item_signals(NULL, '2026-10-06T00:00:00Z')) = 2,
                 '6: NULL items reads every item in the window');
END $$;

-- 7. Freshness.
DO $$
DECLARE f record;
BEGIN
  TRUNCATE personalization.feed_signal_runs;
  SELECT * INTO f FROM personalization.feed_signal_freshness('2026-10-06T14:00:00Z');
  PERFORM assert(f.stale AND f.reason = 'no successful run', '7: no run is stale');

  PERFORM personalization.record_feed_signal_run('failed', 'g7', '2026-10-06T13:00:00Z', 'boom', '2026-10-06T13:55:00Z');
  SELECT * INTO f FROM personalization.feed_signal_freshness('2026-10-06T14:00:00Z');
  PERFORM assert(f.stale, '7: a failed run does not make it fresh');

  INSERT INTO personalization.feed_signal_runs (finished_at, status, generation, data_through)
  VALUES ('2026-10-06T13:10:00Z', 'succeeded', 'g7', '2026-10-06T13:00:00Z');
  SELECT * INTO f FROM personalization.feed_signal_freshness('2026-10-06T14:00:00Z');
  PERFORM assert(NOT f.stale AND f.reason IS NULL, '7: a recent success is fresh');

  -- Analytics stops publishing: runs keep finding nothing new; fresh runs, ageing data.
  PERFORM personalization.record_feed_signal_run('unchanged', 'g7', '2026-10-06T13:00:00Z', NULL, '2026-10-06T19:10:00Z');
  SELECT * INTO f FROM personalization.feed_signal_freshness('2026-10-06T19:20:00Z');
  PERFORM assert(f.stale AND f.reason LIKE 'newest analytics data is%', '7: data older than 6 hours is stale even while runs succeed');

  -- The job stops: no run for over 3 hours.
  SELECT * INTO f FROM personalization.feed_signal_freshness('2026-10-06T22:20:00Z');
  PERFORM assert(f.stale AND f.reason LIKE 'no successful run for%', '7: no run for 3 hours is stale');
END $$;

TRUNCATE spaces, personalization.feed_item_signals_hourly, personalization.feed_user_seen_daily,
         personalization.feed_signal_runs CASCADE;
