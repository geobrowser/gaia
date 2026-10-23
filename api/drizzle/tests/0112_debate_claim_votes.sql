-- Assertions for 0112: claim votes cast in a debate's own UI count toward the debate in Best
-- (GEO-3461 phase 1b).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
--   1. replace_feed_signals stores debate_claim_votes, and a row without it (before analytics
--      116) stores 0.
--   2. The addend is exactly claim_votes_weight × ln(1 + claim votes), summed over the window's
--      hours, inside the existing term.
--   3. Scale: the measured typical count moves a debate by under a unit; the cap still holds.
--   4. Only debates, only the window.
--   5. refresh_debate_watch_scores picks up a debate whose only signal is claim votes.
--   6. Weight 0 turns it off; a negative weight is refused.

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
       watch_plays_weight = 0.25, watch_cap = 8, watch_claim_votes_weight = 0.5
 WHERE id;

-- ---------------------------------------------------------------------------
-- Fixtures: debates of identical shape and age, differing only in their signals.
--   PLAIN  50 impressions, 1000 s, 20 plays, no claim votes.
--   VOTED  PLAIN plus 4 claim votes (testnet's typical count, 2026-10-23), split over two hours.
--   ONLY   claim votes only: 20, no impressions or plays.
--   HUGE   100 impressions, 1,000,000 s, and 1000 claim votes.
--   OLD    20 claim votes, 40 days ago.
--   C1     a claim with 20 claim votes in its rows.
--   R1, R2 written through replace_feed_signals, with and without the field.
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE ids AS SELECT * FROM (VALUES
  ('PLAIN', 'b1120000-0000-4000-8000-000000000101'::uuid),
  ('VOTED', 'b1120000-0000-4000-8000-000000000102'::uuid),
  ('ONLY',  'b1120000-0000-4000-8000-000000000103'::uuid),
  ('HUGE',  'b1120000-0000-4000-8000-000000000104'::uuid),
  ('OLD',   'b1120000-0000-4000-8000-000000000105'::uuid),
  ('R1',    'b1120000-0000-4000-8000-000000000106'::uuid),
  ('R2',    'b1120000-0000-4000-8000-000000000107'::uuid),
  ('C1',    'b1120000-0000-4000-8000-000000000201'::uuid),
  ('S',     'b1120000-0000-4000-8000-0000000000d0'::uuid),
  ('DEBATE', 'fd51f935-2063-4617-be39-7b672b23364c'::uuid),
  ('CLAIM',  '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid)
) AS v(k, id);
CREATE OR REPLACE FUNCTION pg_temp.i(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM ids WHERE ids.k = $1 $$;

CREATE OR REPLACE FUNCTION pg_temp.score(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT ranking_score FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k) $$;
CREATE OR REPLACE FUNCTION pg_temp.watch(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT watch_score FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k) $$;
CREATE OR REPLACE FUNCTION pg_temp.base(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT public.entity_ranking_score(s.quality_score, s.type_weight, s.intrinsic_score,
                                     s.participation_score, s.comment_score,
                                     NULLIF(e.created_at, '')::bigint, c.tau_seconds, c.quality_floor)
  FROM entity_ranking_scores s JOIN entities e ON e.id = s.entity_id CROSS JOIN entity_ranking_config c
  WHERE s.entity_id = pg_temp.i(k) AND c.id $$;
CREATE OR REPLACE FUNCTION pg_temp.near(a numeric, b numeric) RETURNS boolean LANGUAGE sql AS $$
  SELECT abs(a - b) < 1e-9 $$;
CREATE OR REPLACE FUNCTION pg_temp.sig(k text, hours_ago integer, impressions integer, watch_s integer,
                                       plays integer, claim_votes integer) RETURNS void
LANGUAGE sql AS $$
  INSERT INTO personalization.feed_item_signals_hourly
    (item_id, hour, impressions, plays, explore_plays, watch_ms, explore_watch_ms, debate_claim_votes)
  VALUES (pg_temp.i(k), date_trunc('hour', now()) - make_interval(hours => hours_ago), impressions,
          plays, plays, watch_s * 1000::bigint, watch_s * 1000::bigint, claim_votes)
$$;

INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
SELECT id, '1790000000', '0', '1790000000', '0' FROM ids WHERE k NOT IN ('DEBATE','CLAIM','S');

INSERT INTO values (id, entity_id, property_id, space_id, text)
SELECT gen_random_uuid()::text, id, 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid, pg_temp.i('S'), 'fixture ' || k
FROM ids WHERE k NOT IN ('DEBATE','CLAIM','S');

INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
SELECT gen_random_uuid(), gen_random_uuid(), '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid, id,
       pg_temp.i(CASE WHEN k = 'C1' THEN 'CLAIM' ELSE 'DEBATE' END), pg_temp.i('S'), false
FROM ids WHERE k NOT IN ('DEBATE','CLAIM','S');

-- ---------------------------------------------------------------------------
-- 1. replace_feed_signals. Runs first: it replaces every row from its window start on.
-- ---------------------------------------------------------------------------
SELECT personalization.replace_feed_signals(
  'g1',
  date_trunc('day', now(), 'UTC'),
  date_trunc('day', now(), 'UTC') + interval '1 day',
  jsonb_build_array(
    jsonb_build_object('item_id', pg_temp.i('R1'), 'hour', date_trunc('day', now(), 'UTC'),
                       'impressions', 5, 'impressions_top3', 5, 'impressions_4_10', 0,
                       'impressions_11_30', 0, 'impressions_31_plus', 0, 'opens', 0, 'votes', 0,
                       'plays', 1, 'explore_plays', 1, 'watch_ms', 10000, 'explore_watch_ms', 10000,
                       'plays_with_duration', 1, 'watch_fraction_sum', 0.5, 'completed_plays', 0,
                       'debate_claim_votes', 7),
    jsonb_build_object('item_id', pg_temp.i('R2'), 'hour', date_trunc('day', now(), 'UTC'),
                       'impressions', 5, 'impressions_top3', 5, 'impressions_4_10', 0,
                       'impressions_11_30', 0, 'impressions_31_plus', 0, 'opens', 0, 'votes', 0,
                       'plays', 1, 'explore_plays', 1, 'watch_ms', 10000, 'explore_watch_ms', 10000,
                       'plays_with_duration', 1, 'watch_fraction_sum', 0.5, 'completed_plays', 0)),
  '[]'::jsonb, true);

SELECT assert((SELECT debate_claim_votes FROM personalization.feed_item_signals_hourly
               WHERE item_id = pg_temp.i('R1')) = 7,
              '1: replace_feed_signals stores debate_claim_votes');
SELECT assert((SELECT debate_claim_votes FROM personalization.feed_item_signals_hourly
               WHERE item_id = pg_temp.i('R2')) = 0,
              '1: a row from a view without the column stores 0');

SELECT pg_temp.sig('PLAIN', 1, 30, 600, 12, 0), pg_temp.sig('PLAIN', 30, 20, 400, 8, 0);
SELECT pg_temp.sig('VOTED', 1, 30, 600, 12, 3), pg_temp.sig('VOTED', 30, 20, 400, 8, 1);
SELECT pg_temp.sig('ONLY', 2, 0, 0, 0, 20);
SELECT pg_temp.sig('HUGE', 2, 100, 1000000, 10, 1000);
SELECT pg_temp.sig('OLD', 40 * 24, 0, 0, 0, 20);
SELECT pg_temp.sig('C1', 2, 0, 0, 0, 20);

SELECT public.refresh_entity_ranking_scores(ARRAY(SELECT id FROM ids WHERE k NOT IN ('DEBATE','CLAIM','S','ONLY')));

-- ---------------------------------------------------------------------------
-- 2. The addend.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.near(pg_temp.watch('VOTED') - pg_temp.watch('PLAIN'), 0.5 * ln(5::numeric)),
              '2: four claim votes add exactly 0.5 ln(1 + 4), summed over the window''s hours');
SELECT assert(pg_temp.near(pg_temp.score('VOTED'), pg_temp.base('VOTED') + pg_temp.watch('VOTED')),
              '2: and reach the ranking score');
SELECT assert((SELECT claim_votes FROM public.debate_watch_scores(ARRAY[pg_temp.i('VOTED')])) = 4,
              '2: debate_watch_scores reports the count');
SELECT assert(pg_temp.near(pg_temp.watch('R1') - pg_temp.watch('R2'), 0.5 * ln(8::numeric)),
              '2: rows written by replace_feed_signals count the same way');

-- ---------------------------------------------------------------------------
-- 3. Scale.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.watch('VOTED') - pg_temp.watch('PLAIN') < 1,
              '3: the typical count moves a debate by under a unit');
SELECT assert(pg_temp.watch('HUGE') = 8, '3: the term is still capped');

-- ---------------------------------------------------------------------------
-- 4. Only debates, only the window.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.watch('C1') = 0 AND pg_temp.score('C1') = pg_temp.base('C1'),
              '4: a claim with claim votes in its rows is unchanged');
SELECT assert(pg_temp.watch('OLD') = 0, '4: claim votes older than the window count for nothing');

-- ---------------------------------------------------------------------------
-- 5. refresh_debate_watch_scores finds ONLY, whose only signal is claim votes. Its ratio term is
--    ln(((0 + 20 x 10) / (0 + 20)) / 10) = 0, so the whole term is the addend.
-- ---------------------------------------------------------------------------
SELECT public.refresh_debate_watch_scores();
SELECT assert(pg_temp.near(pg_temp.watch('ONLY'), 0.5 * ln(21::numeric)),
              '5: a debate with only claim votes is picked up and scored by them alone');

-- ---------------------------------------------------------------------------
-- 6. Off switch and constraint.
-- ---------------------------------------------------------------------------
UPDATE entity_ranking_config SET watch_claim_votes_weight = 0 WHERE id;
SELECT public.refresh_debate_watch_scores();
SELECT assert(pg_temp.near(pg_temp.watch('VOTED'), pg_temp.watch('PLAIN')),
              '6: weight 0: claim votes count for nothing');
SELECT assert(pg_temp.watch('ONLY') = 0, '6: and a claim-votes-only term is cleared');

DO $$
BEGIN
  UPDATE entity_ranking_config SET watch_claim_votes_weight = -1 WHERE id;
  RAISE EXCEPTION 'FAIL: 6: a negative claim-votes weight was accepted';
EXCEPTION WHEN check_violation THEN
  RAISE NOTICE 'pass: 6: a negative claim-votes weight is refused';
END $$;

-- Cleanup for running by hand.
UPDATE entity_ranking_config
   SET watch_window_days = 30, watch_prior_impressions = 20, watch_reference_seconds = 10,
       watch_ratio_weight = 3.0, watch_completed_weight = 1.0, watch_plays_weight = 0.25, watch_cap = 8,
       watch_claim_votes_weight = 0.5
 WHERE id;
TRUNCATE entities, values, relations, entity_ranking_scores, entity_type_ranking,
         personalization.feed_item_signals_hourly CASCADE;
DELETE FROM personalization.feed_signal_runs WHERE generation = 'g1';
DROP TABLE ids;
