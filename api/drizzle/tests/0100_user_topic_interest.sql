-- Assertions for 0100: per-user topic interest learned from activity (GEO-3088).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
-- One assertion block per acceptance criterion on the ticket, in its order, then the attribution
-- and isolation rules the design depends on:
--   1. Same follows, different activity: the active user gets a higher weight for that topic.
--   2. Agree and disagree raise interest equally.
--   3. A debate moves interest more than a vote, a follow more than either.
--   4. A month-old burst carries about half its weight, with no user action.
--   5. The sweep picks up a new vote; the nightly refit agrees with it, and reports what it missed.
--   6. Spreading lifts a co-occurring topic, but less than the topic itself.
--   7. No activity and no follows: no weights.
--   8. The top contributing activity is retrievable for a card.
--   9. Question debates and Interested / Not interested credit the question's topics.
--  10. Nothing is written to the graph; no follow is created.
--  11. Attribution: removals, DAO-space comments and follows asserted in someone else's space do
--      not count; a reply to a comment credits what the thread is about.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

TRUNCATE entities, values, relations, spaces, user_votes, entity_topic_ranking,
         personalization.user_topic_signals, personalization.topic_cooccurrence,
         personalization.external_interest_signals, personalization.interest_sweep_state,
         personalization.interest_refit_runs CASCADE;

UPDATE personalization.interest_config
   SET half_life_days = 30, spread_fraction = 0.2, spread_max_neighbours = 10,
       cooccurrence_min_support = 3, cooccurrence_max_topics_per_entity = 20,
       sweep_lookback = '1 hour'
 WHERE id;

-- ---------------------------------------------------------------------------
-- Fixtures. "Now" is 2026-10-01 00:00 UTC; everything below is placed relative to it.
--
-- People (Personal spaces):
--   A, B   both follow FOLLOWED. A also votes on four claims about NARROW; B does nothing else.
--   AG, DG agree / disagree on the same claim at the same moment.
--   V, D   one vote / one debate on the same claim at the same moment (compare with a follow).
--   OLD    four votes 30 days ago.
--   NONE   no activity at all.
--   CM     comments: one on a claim, one reply to someone's comment on another claim.
--   Q      a question debate, and Interested / Not interested on questions.
--   RM     only a removed vote.
-- DAO is a DAO space; comments authored there are not anyone's interest.
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE ids AS SELECT * FROM (VALUES
  -- people
  ('A',    'a0000000-0000-4000-8000-00000000000a'::uuid),
  ('B',    'a0000000-0000-4000-8000-00000000000b'::uuid),
  ('AG',   'a0000000-0000-4000-8000-0000000000a1'::uuid),
  ('DG',   'a0000000-0000-4000-8000-0000000000a2'::uuid),
  ('V',    'a0000000-0000-4000-8000-0000000000a3'::uuid),
  ('D',    'a0000000-0000-4000-8000-0000000000a4'::uuid),
  ('OLD',  'a0000000-0000-4000-8000-0000000000a5'::uuid),
  ('NONE', 'a0000000-0000-4000-8000-0000000000a6'::uuid),
  ('CM',   'a0000000-0000-4000-8000-0000000000a7'::uuid),
  ('Q',    'a0000000-0000-4000-8000-0000000000a8'::uuid),
  ('RM',   'a0000000-0000-4000-8000-0000000000a9'::uuid),
  ('DAO',  'a0000000-0000-4000-8000-0000000000d0'::uuid),
  -- topics
  ('FOLLOWED', 'a0000000-0000-4000-8000-000000000101'::uuid),
  ('NARROW',   'a0000000-0000-4000-8000-000000000102'::uuid),
  ('BROAD',    'a0000000-0000-4000-8000-000000000103'::uuid),
  ('OTHER',    'a0000000-0000-4000-8000-000000000104'::uuid),
  ('QT',       'a0000000-0000-4000-8000-000000000105'::uuid),
  ('QT2',      'a0000000-0000-4000-8000-000000000106'::uuid),
  -- claims: k1..k4 about NARROW (and BROAD), k5 about OTHER, k6 untagged
  ('k1', 'a0000000-0000-4000-8000-000000000201'::uuid),
  ('k2', 'a0000000-0000-4000-8000-000000000202'::uuid),
  ('k3', 'a0000000-0000-4000-8000-000000000203'::uuid),
  ('k4', 'a0000000-0000-4000-8000-000000000204'::uuid),
  ('k5', 'a0000000-0000-4000-8000-000000000205'::uuid),
  ('k6', 'a0000000-0000-4000-8000-000000000206'::uuid),
  -- questions
  ('q1', 'a0000000-0000-4000-8000-000000000301'::uuid),
  ('q2', 'a0000000-0000-4000-8000-000000000302'::uuid),
  -- debates, comments
  ('deb1',  'a0000000-0000-4000-8000-000000000401'::uuid),
  ('debq',  'a0000000-0000-4000-8000-000000000402'::uuid),
  ('c1',    'a0000000-0000-4000-8000-000000000501'::uuid),
  ('c2',    'a0000000-0000-4000-8000-000000000502'::uuid),
  ('c3',    'a0000000-0000-4000-8000-000000000503'::uuid),
  ('cdao',  'a0000000-0000-4000-8000-000000000504'::uuid),
  -- a type for ranking rows
  ('CLAIM', 'a0000000-0000-4000-8000-000000000601'::uuid)
) AS v(k, id);
CREATE OR REPLACE FUNCTION pg_temp.i(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM ids WHERE ids.k = $1 $$;
-- Epoch-seconds text, the format kg-indexer writes, for "now" minus some days.
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
  -- kg-indexer upserts the relation's own entity, and bumps its FROM entity's updated_at.
  INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
  VALUES (rid, pg_temp.ago(days_ago), '0', pg_temp.ago(days_ago), '0');
  INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
  VALUES (from_id, pg_temp.ago(days_ago), '0', pg_temp.ago(days_ago), '0')
  ON CONFLICT (id) DO UPDATE SET updated_at = GREATEST(entities.updated_at, EXCLUDED.updated_at);
END $$;
CREATE OR REPLACE FUNCTION pg_temp.vote(who text, what text, vote_type smallint, vote_kind smallint, days_ago double precision)
RETURNS void LANGUAGE sql AS $$
  INSERT INTO user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at)
  VALUES (pg_temp.i(who), pg_temp.i(what), 0, pg_temp.i('DAO'), vote_type, vote_kind, pg_temp.ts(days_ago))
  ON CONFLICT (user_id, object_id, object_type, space_id, vote_kind)
  DO UPDATE SET vote_type = EXCLUDED.vote_type, voted_at = EXCLUDED.voted_at
$$;
-- Weight of one topic for one user, 0 when absent.
CREATE OR REPLACE FUNCTION pg_temp.w(who text, topic text, at timestamptz DEFAULT timestamptz '2026-10-01 00:00:00+00')
RETURNS double precision LANGUAGE sql AS $$
  SELECT coalesce((SELECT weight FROM personalization.user_topic_weights(pg_temp.i(who), 1000, at)
                   WHERE topic_id = pg_temp.i(topic)), 0)
$$;

INSERT INTO spaces (id, type, address)
SELECT id, 'Personal', 'addr-' || k FROM ids
WHERE k IN ('A','B','AG','DG','V','D','OLD','NONE','CM','Q','RM');
INSERT INTO spaces (id, type, address) VALUES (pg_temp.i('DAO'), 'DAO', 'addr-dao');

-- Every topic, claim, question, debate and comment is an entity, created 60 days ago.
INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
SELECT id, pg_temp.ago(60), '0', pg_temp.ago(60), '0' FROM ids
WHERE k NOT IN ('A','B','AG','DG','V','D','OLD','NONE','CM','Q','RM','DAO','CLAIM');

-- Topics are typed Topic (a follow only counts toward a Topic).
SELECT pg_temp.rel(pg_temp.i(t), '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid,
                   '5ef5a586-0f27-4d8e-8f6c-59ae5b3e89e2'::uuid, pg_temp.i('DAO'), 60)
FROM unnest(ARRAY['FOLLOWED','NARROW','BROAD','OTHER','QT','QT2']) t;

-- Claim topics. k1..k4: NARROW. k5: OTHER. q1: QT. q2: QT2.
SELECT pg_temp.rel(pg_temp.i(k), '806d52bc-27e9-4c91-93c0-57978b093351'::uuid, pg_temp.i(t), pg_temp.i('DAO'), 60)
FROM (VALUES ('k1','NARROW'),('k2','NARROW'),('k3','NARROW'),('k4','NARROW'),
             ('k5','OTHER'),('q1','QT'),('q2','QT2')) v(k, t);

-- Co-occurrence source: the For you ranking table, kept apart from the claims above so a spread
-- weight is never mixed up with an own one. NARROW is on 4 entities, all also tagged BROAD; BROAD
-- is on 12 more of its own. Similarity = 4 / sqrt(4 x 16) = 0.5.
INSERT INTO entity_topic_ranking (topic_id, type_id, entity_id, ranking_score)
SELECT pg_temp.i(t), pg_temp.i('CLAIM'), ('a0000000-0000-4000-8000-0000000007' || n)::uuid, 1
FROM (VALUES ('NARROW'),('BROAD')) v(t), (VALUES ('01'),('02'),('03'),('04')) e(n);
INSERT INTO entity_topic_ranking (topic_id, type_id, entity_id, ranking_score)
SELECT pg_temp.i('BROAD'), pg_temp.i('CLAIM'), gen_random_uuid(), 1 FROM generate_series(1, 12);

-- Follows, in the follower's own space. A and B have the same follows.
SELECT pg_temp.rel(pg_temp.i('A'), 'f374b8f2-d331-48a3-a220-ba3648992e93'::uuid, pg_temp.i('FOLLOWED'), pg_temp.i('A'), 50);
SELECT pg_temp.rel(pg_temp.i('B'), 'f374b8f2-d331-48a3-a220-ba3648992e93'::uuid, pg_temp.i('FOLLOWED'), pg_temp.i('B'), 50);
-- A follow of a non-topic (a claim) is not a topic interest.
SELECT pg_temp.rel(pg_temp.i('B'), 'f374b8f2-d331-48a3-a220-ba3648992e93'::uuid, pg_temp.i('k5'), pg_temp.i('B'), 50);
-- A relation in the DAO space claiming RM follows OTHER is not RM's statement.
SELECT pg_temp.rel(pg_temp.i('RM'), 'f374b8f2-d331-48a3-a220-ba3648992e93'::uuid, pg_temp.i('OTHER'), pg_temp.i('DAO'), 1);

-- A: four votes on NARROW claims, both directions, both kinds, two days ago.
SELECT pg_temp.vote('A', 'k1', 0::smallint, 1::smallint, 2);
SELECT pg_temp.vote('A', 'k2', 1::smallint, 1::smallint, 2);
SELECT pg_temp.vote('A', 'k3', 0::smallint, 0::smallint, 2);
SELECT pg_temp.vote('A', 'k4', 1::smallint, 0::smallint, 2);
-- A second vote kind on the same claim is the same engagement, not a second one.
SELECT pg_temp.vote('A', 'k1', 0::smallint, 0::smallint, 2);

-- AG agrees, DG disagrees, same claim, same moment.
SELECT pg_temp.vote('AG', 'k5', 0::smallint, 1::smallint, 3);
SELECT pg_temp.vote('DG', 'k5', 1::smallint, 1::smallint, 3);

-- V votes once; D debates once; same claim (k5), same moment.
SELECT pg_temp.vote('V', 'k5', 0::smallint, 1::smallint, 5);
SELECT pg_temp.rel(pg_temp.i('deb1'), '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid, 'fd51f935-2063-4617-be39-7b672b23364c'::uuid, pg_temp.i('DAO'), 5);
SELECT pg_temp.rel(pg_temp.i('deb1'), 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid, pg_temp.i('k5'), pg_temp.i('DAO'), 5);
SELECT pg_temp.rel(pg_temp.i('deb1'), '0b9b1a35-2068-4431-8f7d-2350f958a728'::uuid, pg_temp.i('D'), pg_temp.i('DAO'), 5);
UPDATE entities SET created_at = pg_temp.ago(5) WHERE id = pg_temp.i('deb1');

-- OLD: four votes exactly 30 days ago.
SELECT pg_temp.vote('OLD', k, 0::smallint, 1::smallint, 30) FROM unnest(ARRAY['k1','k2','k3','k4']) k;

-- RM: only a removed vote.
SELECT pg_temp.vote('RM', 'k5', 2::smallint, 1::smallint, 1);

-- CM: c1 comments on k5. DG comments c2 on k1, and CM's c3 replies to c2, so c3 is about k1.
-- cdao is authored in the DAO space.
SELECT pg_temp.rel(pg_temp.i('c1'), '310d4a24-0e5b-451c-b215-1bfce40d0fe6'::uuid, pg_temp.i('k5'), pg_temp.i('CM'), 1);
SELECT pg_temp.rel(pg_temp.i('c2'), '310d4a24-0e5b-451c-b215-1bfce40d0fe6'::uuid, pg_temp.i('k1'), pg_temp.i('DG'), 1);
SELECT pg_temp.rel(pg_temp.i('c3'), '310d4a24-0e5b-451c-b215-1bfce40d0fe6'::uuid, pg_temp.i('c2'), pg_temp.i('CM'), 1);
SELECT pg_temp.rel(pg_temp.i('cdao'), '310d4a24-0e5b-451c-b215-1bfce40d0fe6'::uuid, pg_temp.i('k5'), pg_temp.i('DAO'), 1);
UPDATE entities SET created_at = pg_temp.ago(1) WHERE id IN (pg_temp.i('c1'), pg_temp.i('c2'), pg_temp.i('c3'), pg_temp.i('cdao'));

-- Q: a question debate on q1; Interested on q1 is recorded too; Not interested on q2.
SELECT pg_temp.rel(pg_temp.i('debq'), '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid, 'fd51f935-2063-4617-be39-7b672b23364c'::uuid, pg_temp.i('DAO'), 0.5);
SELECT pg_temp.rel(pg_temp.i('debq'), 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid, pg_temp.i('q1'), pg_temp.i('DAO'), 0.5);
SELECT pg_temp.rel(pg_temp.i('debq'), '0b9b1a35-2068-4431-8f7d-2350f958a728'::uuid, pg_temp.i('Q'), pg_temp.i('DAO'), 0.5);
UPDATE entities SET created_at = pg_temp.ago(0.5) WHERE id = pg_temp.i('debq');
INSERT INTO personalization.external_interest_signals (user_id, object_id, kind, occurred_at, recorded_at) VALUES
  (pg_temp.i('Q'), pg_temp.i('q1'), 'interested', pg_temp.ts(0.5), pg_temp.ts(0.5)),
  (pg_temp.i('Q'), pg_temp.i('q2'), 'not_interested', pg_temp.ts(0.5), pg_temp.ts(0.5));

CREATE TEMP TABLE graph_before AS
SELECT (SELECT count(*) FROM relations) AS rels,
       (SELECT count(*) FROM relations WHERE type_id = 'f374b8f2-d331-48a3-a220-ba3648992e93'::uuid) AS follows,
       (SELECT count(*) FROM entities) AS ents, (SELECT count(*) FROM values) AS vals;

-- First sweep: no cursor yet, so it covers everyone (the backfill).
SELECT personalization.refresh_topic_cooccurrence();
CREATE TEMP TABLE first_sweep AS SELECT * FROM personalization.sweep_user_topic_interest(timestamptz '2026-10-01 00:00:00+00');

DO $$
DECLARE
  r record;
  w_a double precision; w_b double precision;
  w_ag double precision; w_dg double precision;
  w_v double precision; w_d double precision; w_f double precision;
BEGIN
  SELECT * INTO r FROM first_sweep;
  -- With no cursor yet it is the backfill: all 11 people, NONE included, but not DAO.
  PERFORM assert(r.dirty_users = 11, 'the first sweep covers every person (' || r.dirty_users || ')');

  -- 1. Same follows, different activity.
  w_a := pg_temp.w('A', 'NARROW'); w_b := pg_temp.w('B', 'NARROW');
  PERFORM assert(w_a > 3.5 AND w_b = 0, format('1: A, who voted on 4 NARROW claims, outweighs B on NARROW (%s vs %s)', w_a, w_b));
  PERFORM assert(pg_temp.w('A', 'FOLLOWED') = pg_temp.w('B', 'FOLLOWED') AND pg_temp.w('A', 'FOLLOWED') = 3,
                 '1: their shared follow weighs the same for both, 3.0');
  PERFORM assert(abs(w_a - 4 * power(0.5, 2.0 / 30)) < 1e-9,
                 format('1: four distinct claims at 2 days = 4 x 0.5^(2/30); a second vote kind on k1 adds nothing (%s)', w_a));

  -- 2. Agree = disagree.
  w_ag := pg_temp.w('AG', 'OTHER'); w_dg := pg_temp.w('DG', 'OTHER');
  PERFORM assert(w_ag > 0 AND abs(w_ag - w_dg) < 1e-12, format('2: agree and disagree weigh the same (%s, %s)', w_ag, w_dg));

  -- 3. Debate > vote; follow > both.
  w_v := pg_temp.w('V', 'OTHER'); w_d := pg_temp.w('D', 'OTHER'); w_f := pg_temp.w('A', 'FOLLOWED');
  PERFORM assert(w_d > w_v AND abs(w_d / w_v - 2.0) < 1e-9, format('3: a debate moves interest twice a vote (%s vs %s)', w_d, w_v));
  PERFORM assert(w_f > w_d AND w_f > w_v, format('3: a follow outweighs a debate and a vote (%s)', w_f));
END $$;

-- 4. Old activity fades, with no user action.
DO $$
DECLARE w_now double precision; w_month double precision; w_old double precision;
BEGIN
  w_old := pg_temp.w('OLD', 'NARROW');
  PERFORM assert(abs(w_old - 2.0) < 1e-9, format('4: four votes 30 days old carry half their weight, 2.0 (%s)', w_old));
  -- Read A a month later, nothing recomputed in between.
  w_now := pg_temp.w('A', 'NARROW');
  w_month := pg_temp.w('A', 'NARROW', timestamptz '2026-10-31 00:00:00+00');
  PERFORM assert(abs(w_month / w_now - 0.5) < 1e-9, format('4: a month later A''s NARROW weight has halved (%s -> %s)', w_now, w_month));
  PERFORM assert(pg_temp.w('A', 'FOLLOWED', timestamptz '2026-10-31 00:00:00+00') = 3, '4: a follow does not fade');
END $$;

-- 6. Spreading. OLD's only own topic is NARROW; BROAD co-occurs with it (similarity 0.5).
DO $$
DECLARE r record; n integer; own_narrow double precision;
BEGIN
  SELECT count(*) INTO n FROM personalization.topic_cooccurrence
   WHERE topic_id = pg_temp.i('NARROW') AND neighbour_id = pg_temp.i('BROAD') AND similarity = 0.5 AND support = 4;
  PERFORM assert(n = 1, '6: NARROW and BROAD co-occur, similarity 4/sqrt(4x16) = 0.5');
  own_narrow := pg_temp.w('OLD', 'NARROW');
  SELECT * INTO r FROM personalization.user_topic_weights(pg_temp.i('OLD'), 100, timestamptz '2026-10-01 00:00:00+00')
   WHERE topic_id = pg_temp.i('BROAD');
  PERFORM assert(r.own_weight = 0 AND r.weight > 0, format('6: activity on NARROW lifts BROAD (%s)', r.weight));
  PERFORM assert(r.weight < own_narrow, format('6: ...by less than NARROW itself (%s < %s)', r.weight, own_narrow));
  PERFORM assert(abs(r.weight - 0.2 * 0.5 * own_narrow) < 1e-9, '6: ...namely fraction x similarity x own weight');
  PERFORM assert(r.top_kind = 'related' AND r.top_object_id = pg_temp.i('NARROW') AND r.related_via_topic_id = pg_temp.i('NARROW'),
                 '6: and its reason is "related to NARROW"');
  PERFORM assert(pg_temp.w('OLD', 'OTHER') = 0, '6: an unrelated topic gets nothing');
  PERFORM assert(pg_temp.w('B', 'BROAD') = 0, '6: nor does a user with no activity on the source topic');
END $$;

-- 6. "Too strong": a topic with many close neighbours hands out at most spread_fraction of its
-- weight in all, shared in proportion to similarity.
DO $$
DECLARE total_out double precision; own_narrow double precision; w_qt double precision; w_broad double precision;
BEGIN
  INSERT INTO personalization.topic_cooccurrence (topic_id, neighbour_id, similarity, support) VALUES
    (pg_temp.i('NARROW'), pg_temp.i('QT'), 0.9, 10), (pg_temp.i('NARROW'), pg_temp.i('QT2'), 0.9, 10);
  own_narrow := pg_temp.w('OLD', 'NARROW');
  SELECT sum(related_weight) INTO total_out
  FROM personalization.user_topic_weights(pg_temp.i('OLD'), 100, timestamptz '2026-10-01 00:00:00+00');
  PERFORM assert(abs(total_out - 0.2 * own_narrow) < 1e-9,
                 format('6: similarities summing to 2.3 still spread only 0.2 x own in total (%s of %s)', total_out, own_narrow));
  w_qt := pg_temp.w('OLD', 'QT'); w_broad := pg_temp.w('OLD', 'BROAD');
  PERFORM assert(abs(w_qt / w_broad - 0.9 / 0.5) < 1e-9, '6: ...shared in proportion to similarity');
  DELETE FROM personalization.topic_cooccurrence WHERE neighbour_id IN (pg_temp.i('QT'), pg_temp.i('QT2'));
END $$;

-- 7. No activity, no follows: no weights. Also a user with only a removed vote.
DO $$
DECLARE n integer;
BEGIN
  SELECT count(*) INTO n FROM personalization.user_topic_signals WHERE user_id = pg_temp.i('NONE');
  PERFORM assert(n = 0, '7: no activity and no follows stores nothing');
  SELECT count(*) INTO n FROM personalization.user_topic_weights(pg_temp.i('NONE'));
  PERFORM assert(n = 0, '7: ...and reads nothing, so For you falls back to plain Best');
  SELECT count(*) INTO n FROM personalization.user_topic_signals WHERE user_id = pg_temp.i('RM');
  PERFORM assert(n = 0, '11: a removed vote and a follow stated in another space count for nothing');
END $$;

-- 8. The reason a card can give.
DO $$
DECLARE r record;
BEGIN
  SELECT * INTO r FROM personalization.user_topic_weights(pg_temp.i('A'), 100, timestamptz '2026-10-01 00:00:00+00')
   WHERE topic_id = pg_temp.i('NARROW');
  PERFORM assert(r.top_kind = 'vote' AND r.top_kind_count = 4,
                 format('8: "Because you voted on 4 claims about NARROW" (%s, %s)', r.top_kind, r.top_kind_count));
  PERFORM assert(r.top_object_id IN (pg_temp.i('k1'), pg_temp.i('k2'), pg_temp.i('k3'), pg_temp.i('k4')),
                 '8: with the claim that contributed most');
  SELECT * INTO r FROM personalization.user_topic_interest_reasons(pg_temp.i('D'), pg_temp.i('OTHER'),
                                                                    timestamptz '2026-10-01 00:00:00+00') LIMIT 1;
  PERFORM assert(r.kind = 'debate' AND r.event_count = 1 AND r.top_object_id = pg_temp.i('deb1'),
                 '8: a debater''s reason names the debate');
END $$;

-- 9. Questions, and comments (11).
DO $$
DECLARE w_qt double precision; w_qt2 double precision; n integer;
BEGIN
  w_qt := pg_temp.w('Q', 'QT');
  PERFORM assert(abs(w_qt - 4.0 * power(0.5, 0.5 / 30)) < 1e-9,
                 format('9: a question debate (2.0) and Interested (2.0) credit the question''s topic (%s)', w_qt));
  SELECT count(*) INTO n FROM personalization.user_topic_weights(pg_temp.i('Q')) WHERE topic_id = pg_temp.i('QT2');
  PERFORM assert(n = 0, '9: Not interested alone leaves no positive weight');
  SELECT decayed_count INTO w_qt2 FROM personalization.user_topic_signals
   WHERE user_id = pg_temp.i('Q') AND topic_id = pg_temp.i('QT2') AND kind = 'not_interested';
  PERFORM assert(abs(w_qt2 - power(0.5, 0.5 / 30)) < 1e-12, '9: ...but is stored, and counts against the topic');

  PERFORM assert(abs(pg_temp.w('CM', 'OTHER') - 1.5 * power(0.5, 1.0 / 30)) < 1e-9, '11: a comment on a claim credits its topic at 1.5');
  PERFORM assert(pg_temp.w('CM', 'NARROW') > 0, '11: a reply to a comment credits what the thread is about');
  SELECT count(*) INTO n FROM personalization.user_topic_signals WHERE user_id = pg_temp.i('DAO');
  PERFORM assert(n = 0, '11: a comment authored in a DAO space is nobody''s interest');
END $$;

-- 5. The sweep picks up a new vote within one run; re-running is harmless; the refit agrees.
DO $$
DECLARE r record; before_rows text; after_rows text; w_b double precision;
BEGIN
  -- Idempotent: the same run again rewrites the same rows.
  SELECT string_agg(format('%s/%s/%s/%s/%s', user_id, topic_id, kind, decayed_count, event_count), ',' ORDER BY user_id, topic_id, kind)
    INTO before_rows FROM personalization.user_topic_signals;
  PERFORM personalization.recompute_user_topic_interest(
    ARRAY(SELECT DISTINCT user_id FROM personalization.user_topic_signals), timestamptz '2026-10-01 00:00:00+00');
  SELECT string_agg(format('%s/%s/%s/%s/%s', user_id, topic_id, kind, decayed_count, event_count), ',' ORDER BY user_id, topic_id, kind)
    INTO after_rows FROM personalization.user_topic_signals;
  PERFORM assert(before_rows = after_rows, '5: recomputing the same users at the same time changes nothing');

  -- B votes on a NARROW claim two minutes after the last run.
  INSERT INTO user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at)
  VALUES (pg_temp.i('B'), pg_temp.i('k1'), 0, pg_temp.i('DAO'), 1, 1, timestamptz '2026-10-01 00:01:00+00');
  SELECT * INTO r FROM personalization.sweep_user_topic_interest(timestamptz '2026-10-01 00:02:00+00');
  w_b := pg_temp.w('B', 'NARROW', timestamptz '2026-10-01 00:02:00+00');
  PERFORM assert(w_b > 0.99, format('5: the next sweep picks up B''s new vote (%s)', w_b));
  PERFORM assert(r.dirty_users = 1, format('5: ...recomputing only the users with new activity (%s)', r.dirty_users));
  PERFORM assert(r.since = timestamptz '2026-09-30 23:00:00+00', '5: ...looking back an hour from the last run');

  SELECT * INTO r FROM personalization.refit_user_topic_interest(timestamptz '2026-10-01 03:00:00+00');
  PERFORM assert(r.disagreeing_rows = 0 AND r.max_abs_diff < 1e-9,
                 format('5: the nightly refit agrees with the incremental state (%s rows, max diff %s)', r.disagreeing_rows, r.max_abs_diff));
  -- Everyone with a positive or negative signal: not NONE, RM or DAO.
  PERFORM assert(r.signal_rows > 0 AND r.users = 9, format('5: ...over every user with a signal (%s)', r.users));
END $$;

-- 5. What the sweep cannot see, the refit catches and reports.
DO $$
DECLARE r record; w_before double precision;
BEGIN
  w_before := pg_temp.w('V', 'QT', timestamptz '2026-10-01 04:00:00+00');
  -- k5 (which V voted on) gains a topic. No user did anything, so no sweep notices.
  PERFORM pg_temp.rel(pg_temp.i('k5'), '806d52bc-27e9-4c91-93c0-57978b093351'::uuid, pg_temp.i('QT'), pg_temp.i('DAO'), 60);
  SELECT * INTO r FROM personalization.sweep_user_topic_interest(timestamptz '2026-10-01 04:00:00+00');
  PERFORM assert(pg_temp.w('V', 'QT', timestamptz '2026-10-01 04:00:00+00') = w_before, '5: a re-tagged claim is invisible to the sweep');
  SELECT * INTO r FROM personalization.refit_user_topic_interest(timestamptz '2026-10-01 04:00:00+00');
  PERFORM assert(r.disagreeing_rows >= 3, format('5: the refit reports it as disagreement (%s rows)', r.disagreeing_rows));
  PERFORM assert(pg_temp.w('V', 'QT', timestamptz '2026-10-01 04:00:00+00') > 0, '5: ...and fixes it');
  PERFORM assert((SELECT count(*) FROM personalization.interest_refit_runs) = 2, '5: each refit is recorded');
END $$;

-- 10. Nothing is written to the graph on anyone's behalf.
DO $$
DECLARE g record;
BEGIN
  SELECT * INTO g FROM graph_before;
  -- One relation was added by this suite's own re-tag fixture above.
  PERFORM assert((SELECT count(*) FROM relations) = g.rels + 1, '10: no relation was written by the interest functions');
  PERFORM assert((SELECT count(*) FROM relations WHERE type_id = 'f374b8f2-d331-48a3-a220-ba3648992e93'::uuid) = g.follows,
                 '10: activity never creates a follow');
  PERFORM assert((SELECT count(*) FROM values) = g.vals, '10: no value was written');
END $$;

TRUNCATE entities, values, relations, spaces, user_votes, entity_topic_ranking,
         personalization.user_topic_signals, personalization.topic_cooccurrence,
         personalization.external_interest_signals, personalization.interest_sweep_state,
         personalization.interest_refit_runs CASCADE;
