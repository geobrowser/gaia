-- Assertions for 0114: new debates get a starting lift in Best that fades over their first
-- impressions.
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
--   1. A debate created within the window with no signals starts exactly fresh_boost up, and
--      refresh_debate_watch_scores picks it up although it has no signal rows.
--   2. The boost fades linearly with impressions and is 0 from fresh_impressions on.
--   3. It sits outside the clamp; a debate past the window, and anything not a debate, get none.
--   4. It reaches the ranking score: a new debate with no votes outranks the same debate a week
--      older that kept 0078's participation term worth the typical 19-21 Oct votes.
--   5. Boost 0 turns it off; bad values are refused.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

TRUNCATE entities, values, relations, votes_count, user_votes, spaces, entity_ranking_scores,
         entity_type_weights, entity_type_exclusions, entity_type_ranking, entity_topic_ranking,
         entity_feed_blocklist, personalization.feed_item_signals_hourly CASCADE;

UPDATE entity_ranking_config
   SET participation_weight = 4, participation_cap = 19, comment_weight = 0, comment_cap = 30,
       tau_seconds = 100000, watch_window_days = 30, watch_prior_impressions = 20,
       watch_reference_seconds = 10, watch_ratio_weight = 3.0, watch_completed_weight = 1.0,
       watch_plays_weight = 0.25, watch_cap = 8, watch_claim_votes_weight = 0.5,
       watch_fresh_boost = 5, watch_fresh_impressions = 50
 WHERE id;

-- ---------------------------------------------------------------------------
-- Fixtures. Ages are relative to now; impressions carry exactly the reference 10 s each, so the
-- ratio term is 0 and plays are 0, leaving the boost alone in the watch term.
--   NEW     created 1 hour ago, no signals at all.
--   SEEN10  2 days old, 10 impressions.        SEEN25  25 impressions.
--   SEEN50  50 impressions.                    SEEN80  80 impressions.
--   STALE   40 days old, no signals.
--   HUGE    1 day old, 10 impressions, 1,000,000 s and 1000 completed plays: at the cap.
--   VOTED   8 days old, 3 up + 0 down debate votes (the 19-21 Oct average) and 50 impressions
--           (about what those debates collected), so its own boost has faded.
--   C1      a claim created 1 hour ago.
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE ids AS SELECT * FROM (VALUES
  ('NEW',    'b1140000-0000-4000-8000-000000000101'::uuid, 1),
  ('SEEN10', 'b1140000-0000-4000-8000-000000000102'::uuid, 48),
  ('SEEN25', 'b1140000-0000-4000-8000-000000000103'::uuid, 48),
  ('SEEN50', 'b1140000-0000-4000-8000-000000000104'::uuid, 48),
  ('SEEN80', 'b1140000-0000-4000-8000-000000000105'::uuid, 48),
  ('STALE',  'b1140000-0000-4000-8000-000000000106'::uuid, 40 * 24),
  ('HUGE',   'b1140000-0000-4000-8000-000000000107'::uuid, 24),
  ('VOTED',  'b1140000-0000-4000-8000-000000000108'::uuid, 8 * 24),
  ('C1',     'b1140000-0000-4000-8000-000000000201'::uuid, 1),
  ('S',      'b1140000-0000-4000-8000-0000000000d0'::uuid, 0),
  ('DEBATE', 'fd51f935-2063-4617-be39-7b672b23364c'::uuid, 0),
  ('CLAIM',  '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid, 0)
) AS v(k, id, age_hours);
CREATE OR REPLACE FUNCTION pg_temp.i(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM ids WHERE ids.k = $1 $$;

CREATE OR REPLACE FUNCTION pg_temp.score(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT ranking_score FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k) $$;
CREATE OR REPLACE FUNCTION pg_temp.watch(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT watch_score FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k) $$;
CREATE OR REPLACE FUNCTION pg_temp.near(a numeric, b numeric) RETURNS boolean LANGUAGE sql AS $$
  SELECT abs(a - b) < 1e-9 $$;
CREATE OR REPLACE FUNCTION pg_temp.sig(k text, impressions integer, watch_s integer, completed integer)
RETURNS void LANGUAGE sql AS $$
  INSERT INTO personalization.feed_item_signals_hourly
    (item_id, hour, impressions, plays, explore_plays, watch_ms, explore_watch_ms, completed_plays)
  VALUES (pg_temp.i(k), date_trunc('hour', now()) - interval '1 hour', impressions, 0, 0,
          watch_s * 1000::bigint, watch_s * 1000::bigint, completed)
$$;

INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
SELECT id, floor(extract(epoch FROM now() - make_interval(hours => age_hours)))::bigint::text, '0',
       floor(extract(epoch FROM now()))::bigint::text, '0'
FROM ids WHERE k NOT IN ('DEBATE','CLAIM','S');

INSERT INTO values (id, entity_id, property_id, space_id, text)
SELECT gen_random_uuid()::text, id, 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid, pg_temp.i('S'), 'fixture ' || k
FROM ids WHERE k NOT IN ('DEBATE','CLAIM','S');

INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
SELECT gen_random_uuid(), gen_random_uuid(), '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid, id,
       pg_temp.i(CASE WHEN k = 'C1' THEN 'CLAIM' ELSE 'DEBATE' END), pg_temp.i('S'), false
FROM ids WHERE k NOT IN ('DEBATE','CLAIM','S');

INSERT INTO votes_count (object_id, object_type, vote_kind, space_id, positive, negative)
VALUES (pg_temp.i('VOTED'), 0, 0, pg_temp.i('S'), 3, 0);

SELECT pg_temp.sig('SEEN10', 10, 100, 0), pg_temp.sig('SEEN25', 25, 250, 0),
       pg_temp.sig('SEEN50', 50, 500, 0), pg_temp.sig('SEEN80', 80, 800, 0),
       pg_temp.sig('HUGE', 10, 1000000, 1000),
       pg_temp.sig('VOTED', 50, 500, 0);

-- ---------------------------------------------------------------------------
-- 1. Picked up and scored with no signals.
-- ---------------------------------------------------------------------------
SELECT assert(public.refresh_debate_watch_scores() = 7,
              '1: the hourly re-score takes every debate in the window, signals or not, and STALE');
-- STALE and C1 are outside that set; score them directly to check they get no boost.
SELECT public.refresh_entity_ranking_scores(ARRAY[pg_temp.i('STALE'), pg_temp.i('C1')]);
SELECT assert(pg_temp.near(pg_temp.watch('NEW'), 5), '1: a debate with no signals starts exactly 5 up');

-- ---------------------------------------------------------------------------
-- 2. Linear fade.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.near(pg_temp.watch('SEEN10'), 4), '2: 10 of 50 impressions leaves 4');
SELECT assert(pg_temp.near(pg_temp.watch('SEEN25'), 2.5), '2: 25 leaves 2.5');
SELECT assert(pg_temp.near(pg_temp.watch('SEEN50'), 0), '2: 50 leaves nothing');
SELECT assert(pg_temp.near(pg_temp.watch('SEEN80'), 0), '2: and it never goes negative');

-- ---------------------------------------------------------------------------
-- 3. Outside the clamp; only debates in the window.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.near(pg_temp.watch('HUGE'), 8 + 4), '3: the boost sits on top of the capped term');
SELECT assert(pg_temp.watch('STALE') = 0, '3: a debate older than the window gets none');
SELECT assert(pg_temp.watch('C1') = 0, '3: a new claim gets none');

-- ---------------------------------------------------------------------------
-- 4. Ranking: NEW beats VOTED, which is 8 days older, has used up its boost, and carries
--    4 ln(1 + 3) = 5.5 participation.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.near((SELECT participation_score FROM entity_ranking_scores
                            WHERE entity_id = pg_temp.i('VOTED')), 4 * ln(4::numeric)),
              '4: VOTED carries the typical vote term');
SELECT assert(pg_temp.score('NEW') > pg_temp.score('VOTED'),
              '4: a new debate with no votes outranks it');

UPDATE entity_ranking_config SET watch_fresh_boost = 0 WHERE id;
SELECT public.refresh_debate_watch_scores();
SELECT assert(pg_temp.score('NEW') < pg_temp.score('VOTED'), '4: and without the boost it does not');

-- ---------------------------------------------------------------------------
-- 5. Off switch and constraint.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.watch('NEW') = 0 AND pg_temp.watch('SEEN10') = 0,
              '5: boost 0: the term is 0112''s exactly');

DO $$
BEGIN
  UPDATE entity_ranking_config SET watch_fresh_boost = -1 WHERE id;
  RAISE EXCEPTION 'FAIL: 5: a negative fresh boost was accepted';
EXCEPTION WHEN check_violation THEN
  RAISE NOTICE 'pass: 5: a negative fresh boost is refused';
END $$;
DO $$
BEGIN
  UPDATE entity_ranking_config SET watch_fresh_impressions = 0 WHERE id;
  RAISE EXCEPTION 'FAIL: 5: zero fresh impressions was accepted';
EXCEPTION WHEN check_violation THEN
  RAISE NOTICE 'pass: 5: zero fresh impressions is refused';
END $$;

-- Cleanup for running by hand.
UPDATE entity_ranking_config
   SET watch_window_days = 30, watch_prior_impressions = 20, watch_reference_seconds = 10,
       watch_ratio_weight = 3.0, watch_completed_weight = 1.0, watch_plays_weight = 0.25, watch_cap = 8,
       watch_claim_votes_weight = 0.5, watch_fresh_boost = 5, watch_fresh_impressions = 50
 WHERE id;
TRUNCATE entities, values, relations, votes_count, entity_ranking_scores, entity_type_ranking,
         personalization.feed_item_signals_hourly CASCADE;
DROP TABLE ids;
