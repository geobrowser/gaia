-- Assertions for 0106: Interested (vote_kind 3) is the topic follow (GEO-3158).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
--   1. Interested on a topic is a follow: the follow weight (3.0), not decaying.
--   2. Interested is counted once, even with a (now ignored) Following relation beside it.
--   3. A Following relation alone gives no follow credit: old topic follows are dropped.
--   4. Clearing Interested drops the follow, whatever relation remains.
--   5. Interested on a topic is not ALSO a vote: it credits nothing through the topic's own
--      Topics relations.
--   6. Interested on something that is not a topic stays an engagement at the vote weight.
--   7. The sweep picks up a new Interested and a cleared one.
--   8. Interested moves no ranking: the ranking score and the raw score ignore kind 3, however
--      large its tally.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

TRUNCATE entities, values, relations, spaces, user_votes, votes_count, entity_ranking_scores,
         entity_topic_ranking, entity_type_weights, entity_type_exclusions,
         personalization.user_topic_signals, personalization.topic_cooccurrence,
         personalization.external_interest_signals, personalization.interest_sweep_state,
         personalization.interest_refit_runs CASCADE;

UPDATE personalization.interest_config
   SET half_life_days = 30, spread_fraction = 0.2, spread_max_neighbours = 10,
       cooccurrence_min_support = 3, cooccurrence_max_topics_per_entity = 20,
       sweep_lookback = '1 hour'
 WHERE id;

-- ---------------------------------------------------------------------------
-- Fixtures. "Now" is 2026-10-01 00:00 UTC.
--   I     Interested on T1, 40 days ago (a follow does not fade, so its age must not matter).
--   BOTH  a Following relation AND Interested on T1.
--   REL   a Following relation to T1 only (a pre-GEO-3158 follow, which no longer counts).
--   CLR   Interested on T1, then cleared.
--   CLRF  a Following relation to T1, and an Interested on T1 that was cleared.
--   NT    Interested on a claim k1 (not a topic) tagged T2.
-- T1 is itself tagged with the topic PARENT, so a follow of T1 that leaked into the vote path
-- would show up as weight on PARENT.
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE ids AS SELECT * FROM (VALUES
  ('I',      'b0000000-0000-4000-8000-000000000001'::uuid),
  ('BOTH',   'b0000000-0000-4000-8000-000000000002'::uuid),
  ('REL',    'b0000000-0000-4000-8000-000000000003'::uuid),
  ('CLR',    'b0000000-0000-4000-8000-000000000004'::uuid),
  ('CLRF',   'b0000000-0000-4000-8000-000000000005'::uuid),
  ('NT',     'b0000000-0000-4000-8000-000000000006'::uuid),
  ('LATE',   'b0000000-0000-4000-8000-000000000007'::uuid),
  ('DAO',    'b0000000-0000-4000-8000-0000000000d0'::uuid),
  ('T1',     'b0000000-0000-4000-8000-000000000101'::uuid),
  ('T2',     'b0000000-0000-4000-8000-000000000102'::uuid),
  ('PARENT', 'b0000000-0000-4000-8000-000000000103'::uuid),
  ('k1',     'b0000000-0000-4000-8000-000000000201'::uuid)
) AS v(k, id);
CREATE OR REPLACE FUNCTION pg_temp.i(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM ids WHERE ids.k = $1 $$;
CREATE OR REPLACE FUNCTION pg_temp.ago(days double precision) RETURNS text LANGUAGE sql AS $$
  SELECT floor(extract(epoch FROM timestamptz '2026-10-01 00:00:00+00' - make_interval(secs => days * 86400)))::bigint::text
$$;
CREATE OR REPLACE FUNCTION pg_temp.ts(days double precision) RETURNS timestamptz LANGUAGE sql AS $$
  SELECT timestamptz '2026-10-01 00:00:00+00' - make_interval(secs => days * 86400)
$$;
CREATE OR REPLACE FUNCTION pg_temp.rel(from_id uuid, type_id uuid, to_id uuid, space uuid, days_ago double precision DEFAULT 1)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE rid uuid := gen_random_uuid();
BEGIN
  INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
  VALUES (rid, gen_random_uuid(), type_id, from_id, to_id, space, false);
  INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
  VALUES (rid, pg_temp.ago(days_ago), '0', pg_temp.ago(days_ago), '0');
  INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
  VALUES (from_id, pg_temp.ago(days_ago), '0', pg_temp.ago(days_ago), '0')
  ON CONFLICT (id) DO UPDATE SET updated_at = GREATEST(entities.updated_at, EXCLUDED.updated_at);
END $$;
-- vote_type: 0 = Interested (Up), 2 = cleared. vote_kind 3 unless given.
CREATE OR REPLACE FUNCTION pg_temp.vote(who text, what text, vote_type smallint, days_ago double precision,
                                        vote_kind smallint DEFAULT 3)
RETURNS void LANGUAGE sql AS $$
  INSERT INTO user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at)
  VALUES (pg_temp.i(who), pg_temp.i(what), 0, pg_temp.i('DAO'), vote_type, vote_kind, pg_temp.ts(days_ago))
  ON CONFLICT (user_id, object_id, object_type, space_id, vote_kind)
  DO UPDATE SET vote_type = EXCLUDED.vote_type, voted_at = EXCLUDED.voted_at
$$;
CREATE OR REPLACE FUNCTION pg_temp.w(who text, topic text, at timestamptz DEFAULT timestamptz '2026-10-01 00:00:00+00')
RETURNS double precision LANGUAGE sql AS $$
  SELECT coalesce((SELECT weight FROM personalization.user_topic_weights(pg_temp.i(who), 1000, at)
                   WHERE topic_id = pg_temp.i(topic)), 0)
$$;
CREATE OR REPLACE FUNCTION pg_temp.follow(who text) RETURNS void LANGUAGE sql AS $$
  SELECT pg_temp.rel(pg_temp.i(who), 'f374b8f2-d331-48a3-a220-ba3648992e93'::uuid, pg_temp.i('T1'), pg_temp.i(who), 50)
$$;

INSERT INTO spaces (id, type, address)
SELECT id, 'Personal', 'addr-' || k FROM ids WHERE k IN ('I','BOTH','REL','CLR','CLRF','NT','LATE');
INSERT INTO spaces (id, type, address) VALUES (pg_temp.i('DAO'), 'DAO', 'addr-dao');

INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
SELECT id, pg_temp.ago(60), '0', pg_temp.ago(60), '0' FROM ids WHERE k IN ('T1','T2','PARENT','k1');

-- T1, T2, PARENT are Topics. T1 is tagged PARENT; k1 is tagged T2.
SELECT pg_temp.rel(pg_temp.i(t), '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid,
                   '5ef5a586-0f27-4d8e-8f6c-59ae5b3e89e2'::uuid, pg_temp.i('DAO'), 60)
FROM unnest(ARRAY['T1','T2','PARENT']) t;
SELECT pg_temp.rel(pg_temp.i('T1'), '806d52bc-27e9-4c91-93c0-57978b093351'::uuid, pg_temp.i('PARENT'), pg_temp.i('DAO'), 60);
SELECT pg_temp.rel(pg_temp.i('k1'), '806d52bc-27e9-4c91-93c0-57978b093351'::uuid, pg_temp.i('T2'), pg_temp.i('DAO'), 60);

SELECT pg_temp.vote('I', 'T1', 0::smallint, 40);
SELECT pg_temp.follow('BOTH');
SELECT pg_temp.vote('BOTH', 'T1', 0::smallint, 2);
SELECT pg_temp.follow('REL');
SELECT pg_temp.vote('CLR', 'T1', 2::smallint, 2);
SELECT pg_temp.follow('CLRF');
SELECT pg_temp.vote('CLRF', 'T1', 2::smallint, 2);
SELECT pg_temp.vote('NT', 'k1', 0::smallint, 2);

SELECT personalization.refresh_topic_cooccurrence();
SELECT * FROM personalization.sweep_user_topic_interest(timestamptz '2026-10-01 00:00:00+00');

DO $$
DECLARE r record; n integer;
BEGIN
  -- 1.
  PERFORM assert(pg_temp.w('I', 'T1') = 3, format('1: Interested on a topic weighs as a follow, 3.0 (%s)', pg_temp.w('I', 'T1')));
  PERFORM assert(pg_temp.w('I', 'T1', timestamptz '2027-01-01 00:00:00+00') = 3, '1: ...and does not fade');
  SELECT * INTO r FROM personalization.user_topic_signals WHERE user_id = pg_temp.i('I') AND topic_id = pg_temp.i('T1');
  PERFORM assert(r.kind = 'follow' AND r.event_count = 1, format('1: ...stored as kind follow (%s, %s)', r.kind, r.event_count));

  -- 2.
  PERFORM assert(pg_temp.w('BOTH', 'T1') = 3, format('2: Interested beside a Following relation is one follow, 3.0 not 6.0 (%s)', pg_temp.w('BOTH', 'T1')));
  SELECT count(*) INTO n FROM personalization.user_interest_events(ARRAY[pg_temp.i('BOTH')]) e WHERE e.kind = 'follow';
  PERFORM assert(n = 1, format('2: ...and one follow event (%s)', n));

  -- 3.
  PERFORM assert(pg_temp.w('REL', 'T1') = 0, '3: a Following relation alone gives no follow credit');
  SELECT count(*) INTO n FROM personalization.user_topic_signals WHERE user_id = pg_temp.i('REL');
  PERFORM assert(n = 0, '3: ...and stores nothing');

  -- 4.
  PERFORM assert(pg_temp.w('CLR', 'T1') = 0, '4: a cleared Interested is not a follow');
  SELECT count(*) INTO n FROM personalization.user_topic_signals WHERE user_id = pg_temp.i('CLR');
  PERFORM assert(n = 0, '4: ...and leaves nothing stored');
  PERFORM assert(pg_temp.w('CLRF', 'T1') = 0, '4: clearing Interested unfollows even with a Following relation left');

  -- 5. T1 is tagged PARENT. A follow names its topic directly; it is not a vote on T1.
  PERFORM assert(pg_temp.w('I', 'PARENT') = 0 AND pg_temp.w('BOTH', 'PARENT') = 0,
                 '5: Interested on a topic is not also a vote crediting the topic''s own topics');
  SELECT count(*) INTO n FROM personalization.user_interest_events(ARRAY[pg_temp.i('I')]) e WHERE e.kind = 'vote';
  PERFORM assert(n = 0, '5: ...no vote event at all');

  -- 6.
  PERFORM assert(abs(pg_temp.w('NT', 'T2') - power(0.5, 2.0 / 30)) < 1e-9,
                 format('6: Interested on a non-topic credits its topics at the vote weight, decaying (%s)', pg_temp.w('NT', 'T2')));
  SELECT count(*) INTO n FROM personalization.user_topic_signals WHERE user_id = pg_temp.i('NT') AND kind = 'follow';
  PERFORM assert(n = 0, '6: ...and is not a follow');
END $$;

-- 7. The sweep sees an Interested arrive and go.
DO $$
DECLARE r record;
BEGIN
  PERFORM pg_temp.vote('LATE', 'T1', 0::smallint, 0);
  UPDATE user_votes SET voted_at = timestamptz '2026-10-01 00:01:00+00' WHERE user_id = pg_temp.i('LATE');
  SELECT * INTO r FROM personalization.sweep_user_topic_interest(timestamptz '2026-10-01 00:02:00+00');
  PERFORM assert(r.dirty_users = 1, format('7: the next sweep finds the new Interested (%s dirty)', r.dirty_users));
  PERFORM assert(pg_temp.w('LATE', 'T1', timestamptz '2026-10-01 00:02:00+00') = 3, '7: ...and the user now follows T1');

  UPDATE user_votes SET vote_type = 2, voted_at = timestamptz '2026-10-01 00:03:00+00' WHERE user_id = pg_temp.i('LATE');
  SELECT * INTO r FROM personalization.sweep_user_topic_interest(timestamptz '2026-10-01 00:04:00+00');
  PERFORM assert(pg_temp.w('LATE', 'T1', timestamptz '2026-10-01 00:04:00+00') = 0, '7: clearing it unfollows on the next sweep');

  SELECT * INTO r FROM personalization.refit_user_topic_interest(timestamptz '2026-10-01 03:00:00+00');
  PERFORM assert(r.disagreeing_rows = 0, format('7: the refit agrees with the swept state (%s rows)', r.disagreeing_rows));
END $$;

-- 8. Ranking ignores kind 3. Two named entities with identical curation and stance; one also
-- carries a large Interested tally.
INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block) VALUES
  ('b0000000-0000-4000-8000-000000000901', '1785478598', '0', '1785478598', '0'),
  ('b0000000-0000-4000-8000-000000000902', '1785478598', '0', '1785478598', '0');
INSERT INTO values (id, entity_id, property_id, space_id, text)
SELECT gen_random_uuid(), e, 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid, pg_temp.i('DAO'), 'fixture'
FROM unnest(ARRAY['b0000000-0000-4000-8000-000000000901'::uuid, 'b0000000-0000-4000-8000-000000000902'::uuid]) e;
INSERT INTO votes_count (object_id, object_type, space_id, vote_kind, positive, negative)
SELECT e, 0, pg_temp.i('DAO'), k, 5, 1
FROM unnest(ARRAY['b0000000-0000-4000-8000-000000000901'::uuid, 'b0000000-0000-4000-8000-000000000902'::uuid]) e,
     unnest(ARRAY[0, 1]::smallint[]) k;
INSERT INTO votes_count (object_id, object_type, space_id, vote_kind, positive, negative)
VALUES ('b0000000-0000-4000-8000-000000000902', 0, pg_temp.i('DAO'), 3, 500, 0);

SELECT public.refresh_entity_ranking_scores(ARRAY['b0000000-0000-4000-8000-000000000901'::uuid,
                                                  'b0000000-0000-4000-8000-000000000902'::uuid]);
DO $$
DECLARE a record; b record; raw_b bigint;
BEGIN
  SELECT * INTO a FROM entity_ranking_scores WHERE entity_id = 'b0000000-0000-4000-8000-000000000901';
  SELECT * INTO b FROM entity_ranking_scores WHERE entity_id = 'b0000000-0000-4000-8000-000000000902';
  PERFORM assert(a.ranking_score = b.ranking_score AND a.participation_score = b.participation_score
                 AND a.quality_score = b.quality_score,
                 format('8: 500 Interested move neither ranking, participation nor quality (%s vs %s)', a.ranking_score, b.ranking_score));
  PERFORM assert(b.positive = 5, format('8: the curation tally on the score row is curation only (%s)', b.positive));
  -- Equal curation ties them on the raw score, and the tiebreak is id ASC, so 902 comes after
  -- 901. Counting its Interested tally would put it first.
  SELECT (SELECT e.ordinality FROM public.entities_ordered_by_score('raw', pg_temp.i('DAO'), 'DESC') WITH ORDINALITY e(id) WHERE e.id = 'b0000000-0000-4000-8000-000000000902')
       - (SELECT e.ordinality FROM public.entities_ordered_by_score('raw', pg_temp.i('DAO'), 'DESC') WITH ORDINALITY e(id) WHERE e.id = 'b0000000-0000-4000-8000-000000000901')
    INTO raw_b;
  PERFORM assert(raw_b > 0, format('8: the raw (curation) order ignores the Interested tally (902 is %s places after 901)', raw_b));
END $$;

TRUNCATE entities, values, relations, spaces, user_votes, votes_count, entity_ranking_scores,
         entity_topic_ranking, entity_type_weights, entity_type_exclusions,
         personalization.user_topic_signals, personalization.topic_cooccurrence,
         personalization.external_interest_signals, personalization.interest_sweep_state,
         personalization.interest_refit_runs CASCADE;
