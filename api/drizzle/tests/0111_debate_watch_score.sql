-- Assertions for 0111: Best scores debates on watch per impression, completed plays and plays
-- (GEO-3461 phase 1).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
--   1. No signals: watch term 0, score exactly 0107's.
--   2. The term is the documented expression, added to the ordinary score.
--   3. Order of importance: watch per impression beats plays; completed plays add exactly
--      ln(1 + completed) at weight 1.
--   4. Smoothing: a few lucky impressions cannot outrank a debate watched well at volume.
--   5. Scale, both directions: a typical debate moves less than a unit, the busiest few by 2-5
--      units; the cap holds.
--   6. Only debates, only the window: a claim with signal rows is untouched; signals older than
--      watch_window_days count for nothing.
--   7. refresh_debate_watch_scores picks up new signals and clears terms whose signals aged out.
--   8. Every weight 0 turns it off.

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
       watch_plays_weight = 0.25, watch_cap = 8
 WHERE id;

-- ---------------------------------------------------------------------------
-- Fixtures: debates of identical shape and age, differing only in their signals (30 days).
--   NONE   no signals.
--   MID    50 impressions, 1000 s on Explore (20 s each), 20 plays, no completed.
--   DONE   MID plus 5 completed plays.
--   SKIP   200 impressions, 400 s (2 s each), 100 plays: shown a lot, scrolled past, autoplayed.
--   FEW    3 impressions, 120 s (40 s each), 3 plays.
--   WELL   200 impressions, 4000 s (20 s each), 3 plays.
--   TYP    testnet's median debate: 5 impressions, 50 s, 9 plays.
--   BUSY   testnet's p99: 254 impressions, 2500 s, 327 plays, 6 completed.
--   HUGE   100 impressions, 1,000,000 s.
--   OLD    WELL's signals, 40 days ago.
--   C1     a claim with WELL's signals.
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE ids AS SELECT * FROM (VALUES
  ('NONE', 'b1110000-0000-4000-8000-000000000101'::uuid),
  ('MID',  'b1110000-0000-4000-8000-000000000102'::uuid),
  ('DONE', 'b1110000-0000-4000-8000-000000000103'::uuid),
  ('SKIP', 'b1110000-0000-4000-8000-000000000104'::uuid),
  ('FEW',  'b1110000-0000-4000-8000-000000000105'::uuid),
  ('WELL', 'b1110000-0000-4000-8000-000000000106'::uuid),
  ('TYP',  'b1110000-0000-4000-8000-000000000107'::uuid),
  ('BUSY', 'b1110000-0000-4000-8000-000000000108'::uuid),
  ('HUGE', 'b1110000-0000-4000-8000-000000000109'::uuid),
  ('OLD',  'b1110000-0000-4000-8000-00000000010a'::uuid),
  ('C1',   'b1110000-0000-4000-8000-000000000201'::uuid),
  ('S',    'b1110000-0000-4000-8000-0000000000d0'::uuid),
  ('DEBATE', 'fd51f935-2063-4617-be39-7b672b23364c'::uuid),
  ('CLAIM',  '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid)
) AS v(k, id);
CREATE OR REPLACE FUNCTION pg_temp.i(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM ids WHERE ids.k = $1 $$;

CREATE OR REPLACE FUNCTION pg_temp.score(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT ranking_score FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k) $$;
CREATE OR REPLACE FUNCTION pg_temp.watch(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT watch_score FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k) $$;
-- 0107's expression for a stored row (none of these is a topic debate).
CREATE OR REPLACE FUNCTION pg_temp.base(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT public.entity_ranking_score(s.quality_score, s.type_weight, s.intrinsic_score,
                                     s.participation_score, s.comment_score,
                                     NULLIF(e.created_at, '')::bigint, c.tau_seconds, c.quality_floor)
  FROM entity_ranking_scores s JOIN entities e ON e.id = s.entity_id CROSS JOIN entity_ranking_config c
  WHERE s.entity_id = pg_temp.i(k) AND c.id $$;
CREATE OR REPLACE FUNCTION pg_temp.near(a numeric, b numeric) RETURNS boolean LANGUAGE sql AS $$
  SELECT abs(a - b) < 1e-9 $$;
CREATE OR REPLACE FUNCTION pg_temp.sig(k text, hours_ago integer, impressions integer, watch_s integer,
                                       plays integer, completed integer) RETURNS void
LANGUAGE sql AS $$
  INSERT INTO personalization.feed_item_signals_hourly
    (item_id, hour, impressions, plays, explore_plays, watch_ms, explore_watch_ms, completed_plays)
  VALUES (pg_temp.i(k), date_trunc('hour', now()) - make_interval(hours => hours_ago), impressions,
          plays, plays, watch_s * 1000::bigint, watch_s * 1000::bigint, completed)
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

-- Split across hours where it matters, so the sums are exercised.
SELECT pg_temp.sig('MID', 1, 30, 600, 12, 0), pg_temp.sig('MID', 30, 20, 400, 8, 0);
SELECT pg_temp.sig('DONE', 1, 30, 600, 12, 3), pg_temp.sig('DONE', 30, 20, 400, 8, 2);
SELECT pg_temp.sig('SKIP', 2, 200, 400, 100, 0);
SELECT pg_temp.sig('FEW', 2, 3, 120, 3, 0);
SELECT pg_temp.sig('WELL', 2, 200, 4000, 3, 0);
SELECT pg_temp.sig('TYP', 2, 5, 50, 9, 0);
SELECT pg_temp.sig('BUSY', 2, 254, 2500, 327, 6);
SELECT pg_temp.sig('HUGE', 2, 100, 1000000, 10, 0);
SELECT pg_temp.sig('OLD', 40 * 24, 200, 4000, 3, 0);
SELECT pg_temp.sig('C1', 2, 200, 4000, 3, 0);

SELECT public.refresh_entity_ranking_scores(ARRAY(SELECT id FROM ids WHERE k NOT IN ('DEBATE','CLAIM','S')));

-- ---------------------------------------------------------------------------
-- 1. No signals.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.watch('NONE') = 0, '1: a debate with no signals has no watch term');
SELECT assert(pg_temp.score('NONE') = pg_temp.base('NONE'), '1: and exactly 0107''s score');

-- ---------------------------------------------------------------------------
-- 2. The expression: 3 ln(((1000 + 20 x 10) / (50 + 20)) / 10) + 0.25 ln(1 + 20).
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.near(pg_temp.watch('MID'), 3 * ln((1200.0 / 70) / 10) + 0.25 * ln(21::numeric)),
              '2: watch term is the documented expression, summed over the window''s hours');
SELECT assert(pg_temp.near(pg_temp.score('MID'), pg_temp.base('MID') + pg_temp.watch('MID')),
              '2: and is added to the ordinary score');
SELECT assert(EXISTS (SELECT 1 FROM entity_type_ranking
                      WHERE type_id = pg_temp.i('DEBATE') AND entity_id = pg_temp.i('MID')
                        AND ranking_score = pg_temp.score('MID')),
              '2: Debate''s type ranking (Explore''s debate slot) carries it');

-- ---------------------------------------------------------------------------
-- 3. Order of importance.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.score('MID') > pg_temp.score('NONE') AND pg_temp.score('NONE') > pg_temp.score('SKIP'),
              '3: watched beats unseen beats scrolled past, even with 100 plays');
SELECT assert(pg_temp.near(pg_temp.watch('DONE') - pg_temp.watch('MID'), ln(6::numeric)),
              '3: five completed plays add exactly ln(1 + 5)');

-- ---------------------------------------------------------------------------
-- 4. Smoothing. FEW's raw 40 s per impression is twice WELL's 20 s, from 3 impressions.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.score('WELL') > pg_temp.score('FEW'),
              '4: 3 impressions at 40 s each do not outrank 200 at 20 s each');
SELECT assert(pg_temp.watch('FEW') > 0, '4: but a good small sample still helps');

-- ---------------------------------------------------------------------------
-- 5. Scale. One unit is tau (about 28 hours) of recency.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.watch('TYP') > 0 AND pg_temp.watch('TYP') < 1,
              '5: the median debate moves by less than a unit');
SELECT assert(pg_temp.watch('BUSY') > 2 AND pg_temp.watch('BUSY') < 5,
              '5: the busiest debates gain 2-5 units, not a week''s worth');
SELECT assert(pg_temp.watch('HUGE') = 8, '5: the term is capped');
SELECT assert(pg_temp.watch('SKIP') > -8 AND pg_temp.watch('SKIP') < -1,
              '5: a debate people scroll past loses more than a unit, short of the cap');

-- ---------------------------------------------------------------------------
-- 6. Only debates, only the window.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.watch('C1') = 0 AND pg_temp.score('C1') = pg_temp.base('C1'),
              '6: a claim with signal rows is unchanged');
SELECT assert(NOT EXISTS (SELECT 1 FROM public.debate_watch_scores(ARRAY[pg_temp.i('C1')])),
              '6: debate_watch_scores returns nothing for a claim');
SELECT assert(pg_temp.watch('OLD') = 0, '6: signals older than the window count for nothing');

-- ---------------------------------------------------------------------------
-- 7. refresh_debate_watch_scores: NONE gets signals, WELL's age out.
-- ---------------------------------------------------------------------------
SELECT pg_temp.sig('NONE', 3, 50, 1000, 20, 0);
UPDATE personalization.feed_item_signals_hourly
   SET hour = hour - interval '40 days' WHERE item_id = pg_temp.i('WELL');

CREATE TEMP TABLE refreshed AS SELECT public.refresh_debate_watch_scores() AS n;
SELECT assert(pg_temp.near(pg_temp.watch('NONE'), pg_temp.watch('MID')),
              '7: new signals reach the score');
SELECT assert(pg_temp.watch('WELL') = 0, '7: a term whose signals aged out is cleared');
-- With signals in the window: MID DONE SKIP FEW TYP BUSY HUGE NONE; nonzero term: WELL. Not OLD or C1.
SELECT assert((SELECT n FROM refreshed) = 9,
              '7: re-scores exactly the debates whose term can have moved, and not the claim');

-- ---------------------------------------------------------------------------
-- 8. Off switch.
-- ---------------------------------------------------------------------------
UPDATE entity_ranking_config
   SET watch_ratio_weight = 0, watch_completed_weight = 0, watch_plays_weight = 0 WHERE id;
SELECT public.refresh_debate_watch_scores();
SELECT assert(NOT EXISTS (SELECT 1 FROM entity_ranking_scores WHERE watch_score <> 0),
              '8: every weight 0: no watch term anywhere');
SELECT assert((SELECT bool_and(s.ranking_score = pg_temp.base(ids.k))
               FROM ids JOIN entity_ranking_scores s ON s.entity_id = ids.id),
              '8: and every score is exactly 0107''s');

-- Cleanup for running by hand.
UPDATE entity_ranking_config
   SET watch_window_days = 30, watch_prior_impressions = 20, watch_reference_seconds = 10,
       watch_ratio_weight = 3.0, watch_completed_weight = 1.0, watch_plays_weight = 0.25, watch_cap = 8
 WHERE id;
TRUNCATE entities, values, relations, entity_ranking_scores, entity_type_ranking,
         personalization.feed_item_signals_hourly CASCADE;
DROP TABLE ids, refreshed;
