-- Assertions for 0105: Not interested (GEO-2862, synced from geo-chat) counts against a claim's
-- topics, through 0100's external_interest_signals (GEO-3088).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
--   1. A snapshot lowers the marker's weight for the claim's topic by exactly the Not interested
--      weight, at once (no sweep needed), and touches nobody else.
--   2. Rows for a non-personal space, or a space gaia does not know, are dropped and counted.
--   3. The same snapshot again changes nothing and recomputes nobody.
--   4. An undo (the row missing from the next snapshot) restores the weight at once.
--   5. A mark cleared and made again comes back with its new time.
--   6. A snapshot that places no row (a broken identity mapping) cannot erase existing rows unless
--      explicitly allowed; an empty snapshot (everyone cleared) is honoured; an unknown kind fails.
--   7. The nightly refit agrees with what the sync left.
--   8. For you leaves out a claim marked Not interested; a held position still reads 'voted'.
--   9. Other kinds in the hook table are untouched by a not_interested snapshot.

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
UPDATE personalization.interest_signal_weights SET weight = 1.0 WHERE kind = 'vote';
UPDATE personalization.interest_signal_weights SET weight = 2.0 WHERE kind = 'interested';
UPDATE personalization.interest_signal_weights SET weight = -3.0 WHERE kind = 'not_interested';

-- "Now" is 2026-10-01 00:00 UTC.
--   N   votes on four NARROW claims (k1..k4) and one OTHER claim (k5), two days ago; then marks
--       k1 Not interested a day ago.
--   P   votes on the same four NARROW claims; marks nothing.
--   DAO a DAO space; GHOST a space gaia has never seen. Both appear in the snapshot.
CREATE TEMP TABLE ids AS SELECT * FROM (VALUES
  ('N',      'b0000000-0000-4000-8000-00000000000a'::uuid),
  ('P',      'b0000000-0000-4000-8000-00000000000b'::uuid),
  ('DAO',    'b0000000-0000-4000-8000-0000000000d0'::uuid),
  ('GHOST',  'b0000000-0000-4000-8000-0000000000e0'::uuid),
  ('NARROW', 'b0000000-0000-4000-8000-000000000101'::uuid),
  ('OTHER',  'b0000000-0000-4000-8000-000000000102'::uuid),
  ('QT',     'b0000000-0000-4000-8000-000000000103'::uuid),
  ('k1',     'b0000000-0000-4000-8000-000000000201'::uuid),
  ('k2',     'b0000000-0000-4000-8000-000000000202'::uuid),
  ('k3',     'b0000000-0000-4000-8000-000000000203'::uuid),
  ('k4',     'b0000000-0000-4000-8000-000000000204'::uuid),
  ('k5',     'b0000000-0000-4000-8000-000000000205'::uuid),
  ('q1',     'b0000000-0000-4000-8000-000000000301'::uuid)
) AS v(k, id);
CREATE OR REPLACE FUNCTION pg_temp.i(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM ids WHERE ids.k = $1 $$;
CREATE OR REPLACE FUNCTION pg_temp.ago(days double precision) RETURNS text LANGUAGE sql AS $$
  SELECT floor(extract(epoch FROM timestamptz '2026-10-01 00:00:00+00' - make_interval(secs => days * 86400)))::bigint::text
$$;
CREATE OR REPLACE FUNCTION pg_temp.ts(days double precision) RETURNS timestamptz LANGUAGE sql AS $$
  SELECT timestamptz '2026-10-01 00:00:00+00' - make_interval(secs => days * 86400)
$$;
CREATE OR REPLACE FUNCTION pg_temp.w(who text, topic text) RETURNS double precision LANGUAGE sql AS $$
  SELECT coalesce((SELECT weight FROM personalization.user_topic_weights(pg_temp.i(who), 1000, pg_temp.ts(0))
                   WHERE topic_id = pg_temp.i(topic)), 0)
$$;
-- One snapshot row, as the sync sends it.
CREATE OR REPLACE FUNCTION pg_temp.mark(who text, what text, days_ago double precision) RETURNS jsonb LANGUAGE sql AS $$
  SELECT jsonb_build_object('user_id', pg_temp.i(who), 'object_id', pg_temp.i(what), 'occurred_at', pg_temp.ts(days_ago))
$$;
CREATE OR REPLACE FUNCTION pg_temp.sync(rows jsonb, allow_unplaced boolean DEFAULT false)
RETURNS TABLE (rows_in integer, placed integer, inserted integer, updated integer, removed integer,
               users_recomputed integer, signal_rows integer)
LANGUAGE sql AS $$
  SELECT * FROM personalization.replace_external_interest_signals('not_interested', rows, allow_unplaced, pg_temp.ts(0))
$$;

INSERT INTO spaces (id, type, address) VALUES
  (pg_temp.i('N'), 'Personal', 'addr-n'), (pg_temp.i('P'), 'Personal', 'addr-p'),
  (pg_temp.i('DAO'), 'DAO', 'addr-dao');

INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
SELECT id, pg_temp.ago(60), '0', pg_temp.ago(60), '0' FROM ids WHERE k NOT IN ('N','P','DAO','GHOST');

INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
SELECT gen_random_uuid(), gen_random_uuid(), '806d52bc-27e9-4c91-93c0-57978b093351'::uuid,
       pg_temp.i(c), pg_temp.i(t), pg_temp.i('DAO'), false
FROM (VALUES ('k1','NARROW'),('k2','NARROW'),('k3','NARROW'),('k4','NARROW'),('k5','OTHER'),('q1','QT')) v(c, t);

INSERT INTO user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at)
SELECT pg_temp.i(who), pg_temp.i(c), 0, pg_temp.i('DAO'), 0, 1, pg_temp.ts(2)
FROM (VALUES ('N','k1'),('N','k2'),('N','k3'),('N','k4'),('N','k5'),
             ('P','k1'),('P','k2'),('P','k3'),('P','k4')) v(who, c);

-- An Interested row for P, which a not_interested snapshot must leave alone.
INSERT INTO personalization.external_interest_signals (user_id, object_id, kind, occurred_at)
VALUES (pg_temp.i('P'), pg_temp.i('q1'), 'interested', pg_temp.ts(1));

-- Backfill, as the first sweep does.
SELECT * FROM personalization.sweep_user_topic_interest(pg_temp.ts(0));

CREATE TEMP TABLE baseline AS
SELECT pg_temp.w('N', 'NARROW') AS n_narrow, pg_temp.w('N', 'OTHER') AS n_other,
       pg_temp.w('P', 'NARROW') AS p_narrow, pg_temp.w('P', 'QT') AS p_qt;

DO $$
DECLARE b record;
BEGIN
  SELECT * INTO b FROM baseline;
  PERFORM assert(abs(b.n_narrow - 4 * power(0.5, 2.0 / 30)) < 1e-9, format('fixture: four votes on NARROW (%s)', b.n_narrow));
  PERFORM assert(b.n_narrow = b.p_narrow, 'fixture: N and P start level on NARROW');
  PERFORM assert(b.p_qt > 0, 'fixture: P is Interested in QT');
END $$;

-- 1, 2. N marks k1 Not interested. The snapshot also carries a DAO space and an unknown space.
DO $$
DECLARE r record; b record; drop_by double precision;
BEGIN
  SELECT * INTO b FROM baseline;
  SELECT * INTO r FROM pg_temp.sync(jsonb_build_array(
    pg_temp.mark('N', 'k1', 1), pg_temp.mark('DAO', 'k2', 1), pg_temp.mark('GHOST', 'k3', 1)));
  PERFORM assert(r.rows_in = 3 AND r.placed = 1, format('2: only the personal space is placed (%s in, %s placed)', r.rows_in, r.placed));
  PERFORM assert(r.inserted = 1 AND r.updated = 0 AND r.removed = 0, '1: one row inserted');
  PERFORM assert(r.users_recomputed = 1, '1: only N is recomputed');

  drop_by := b.n_narrow - pg_temp.w('N', 'NARROW');
  PERFORM assert(abs(drop_by - 3 * power(0.5, 1.0 / 30)) < 1e-9,
                 format('1: N''s NARROW weight drops by the Not interested weight, decayed one day (%s)', drop_by));
  PERFORM assert(pg_temp.w('N', 'NARROW') > 0, '1: ...and four votes still outweigh one Not interested');
  PERFORM assert(pg_temp.w('N', 'OTHER') = b.n_other, '1: a topic the claim is not about is untouched');
  PERFORM assert(pg_temp.w('P', 'NARROW') = b.p_narrow, '1: someone else''s weight is untouched');
  PERFORM assert(NOT EXISTS (SELECT 1 FROM personalization.external_interest_signals
                             WHERE user_id IN (pg_temp.i('DAO'), pg_temp.i('GHOST'))),
                 '2: nothing stored for the DAO or unknown space');
  PERFORM assert((SELECT top_object_id FROM personalization.user_topic_signals
                  WHERE user_id = pg_temp.i('N') AND topic_id = pg_temp.i('NARROW') AND kind = 'not_interested')
                 = pg_temp.i('k1'), '1: the stored signal names the claim');
END $$;

-- 3. The same snapshot again: nothing to do.
DO $$
DECLARE r record; w_before double precision := pg_temp.w('N', 'NARROW');
BEGIN
  SELECT * INTO r FROM pg_temp.sync(jsonb_build_array(pg_temp.mark('N', 'k1', 1)));
  PERFORM assert(r.inserted = 0 AND r.updated = 0 AND r.removed = 0 AND r.users_recomputed = 0,
                 format('3: a repeated snapshot changes nothing (%s/%s/%s, %s users)', r.inserted, r.updated, r.removed, r.users_recomputed));
  PERFORM assert(pg_temp.w('N', 'NARROW') = w_before, '3: ...and the weight is the same');
END $$;

-- 7. The nightly refit agrees with what the sync left.
DO $$
DECLARE r record;
BEGIN
  SELECT * INTO r FROM personalization.refit_user_topic_interest(pg_temp.ts(0), NULL);
  PERFORM assert(r.disagreeing_rows = 0, format('7: refit agrees with the sync (%s rows disagree)', r.disagreeing_rows));
END $$;

-- 8. For you: k1 is left out for N as not_interested; k2 (voted) still reads voted even when also
-- marked; P sees k1 normally apart from P's own vote.
DO $$
DECLARE ex text;
BEGIN
  SELECT excluded INTO ex FROM personalization.for_you_candidates(pg_temp.i('N'), ARRAY[pg_temp.i('q1')]);
  PERFORM assert(ex IS NULL, '8: an unmarked, unanswered candidate is kept');
  -- N has a stance vote on k1, so 'voted' wins; mark q1 (no vote) to see the new reason.
  INSERT INTO personalization.external_interest_signals (user_id, object_id, kind, occurred_at)
  VALUES (pg_temp.i('N'), pg_temp.i('q1'), 'not_interested', pg_temp.ts(1));
  SELECT excluded INTO ex FROM personalization.for_you_candidates(pg_temp.i('N'), ARRAY[pg_temp.i('q1')]);
  PERFORM assert(ex = 'not_interested', format('8: a claim marked Not interested is left out of For you (%s)', ex));
  SELECT excluded INTO ex FROM personalization.for_you_candidates(pg_temp.i('N'), ARRAY[pg_temp.i('k1')]);
  PERFORM assert(ex = 'voted', '8: a held position still reads voted');
  DELETE FROM personalization.external_interest_signals
   WHERE user_id = pg_temp.i('N') AND object_id = pg_temp.i('q1');
END $$;

-- 4. Undo: k1 is missing from the next snapshot. The weight is back at once, without a sweep.
DO $$
DECLARE r record; b record;
BEGIN
  SELECT * INTO b FROM baseline;
  SELECT * INTO r FROM pg_temp.sync(jsonb_build_array(pg_temp.mark('N', 'k2', 1)), false);
  PERFORM assert(r.removed = 1 AND r.inserted = 1, format('4: k1 removed, k2 added (%s removed, %s inserted)', r.removed, r.inserted));
  SELECT * INTO r FROM pg_temp.sync(jsonb_build_array(pg_temp.mark('N', 'k5', 1)), false);
  PERFORM assert(abs(pg_temp.w('N', 'NARROW') - b.n_narrow) < 1e-9, '4: after the undo N''s NARROW weight is restored');
  PERFORM assert(pg_temp.w('N', 'OTHER') = 0, '4: OTHER, one vote against one Not interested, has no positive weight');
END $$;

-- 5. Cleared and made again between syncs: same row, new time.
DO $$
DECLARE r record; t timestamptz;
BEGIN
  SELECT * INTO r FROM pg_temp.sync(jsonb_build_array(pg_temp.mark('N', 'k5', 0.25)));
  PERFORM assert(r.updated = 1 AND r.inserted = 0 AND r.removed = 0 AND r.users_recomputed = 1, '5: a re-mark moves the time');
  SELECT occurred_at INTO t FROM personalization.external_interest_signals
   WHERE user_id = pg_temp.i('N') AND object_id = pg_temp.i('k5') AND kind = 'not_interested';
  PERFORM assert(t = pg_temp.ts(0.25), '5: ...to the new mark''s time');
END $$;

-- 6. A snapshot whose rows land on no personal space cannot erase everything by accident; an
-- empty one (everyone cleared their marks) is honoured; an unknown kind is an error.
DO $$
DECLARE refused boolean := false; r record;
BEGIN
  BEGIN
    PERFORM * FROM pg_temp.sync(jsonb_build_array(pg_temp.mark('GHOST', 'k1', 1), pg_temp.mark('DAO', 'k2', 1)));
  EXCEPTION WHEN raise_exception THEN refused := true;
  END;
  PERFORM assert(refused, '6: a snapshot that places no rows is refused while rows exist');
  PERFORM assert(EXISTS (SELECT 1 FROM personalization.external_interest_signals WHERE kind = 'not_interested'),
                 '6: ...and nothing was removed');

  refused := false;
  BEGIN
    PERFORM * FROM personalization.replace_external_interest_signals('no_such_kind', '[]'::jsonb);
  EXCEPTION WHEN raise_exception THEN refused := true;
  END;
  PERFORM assert(refused, '6: an unknown kind is an error');

  SELECT * INTO r FROM pg_temp.sync(jsonb_build_array(pg_temp.mark('GHOST', 'k1', 1)), true);
  PERFORM assert(r.removed = 1 AND r.users_recomputed = 1, '6: ...unless explicitly allowed');

  SELECT * INTO r FROM pg_temp.sync(jsonb_build_array(pg_temp.mark('N', 'k5', 1)));
  SELECT * INTO r FROM pg_temp.sync('[]'::jsonb);
  PERFORM assert(r.removed = 1 AND r.users_recomputed = 1, '6: an empty snapshot clears the kind (everyone cleared)');
  PERFORM assert(abs(pg_temp.w('N', 'NARROW') - (SELECT n_narrow FROM baseline)) < 1e-9, '6: ...and weights return to the votes alone');
END $$;

-- 9. The Interested row was never touched by any not_interested snapshot.
DO $$
DECLARE b record;
BEGIN
  SELECT * INTO b FROM baseline;
  PERFORM assert(EXISTS (SELECT 1 FROM personalization.external_interest_signals
                         WHERE user_id = pg_temp.i('P') AND kind = 'interested'),
                 '9: other kinds survive a not_interested snapshot');
  PERFORM assert(pg_temp.w('P', 'QT') = b.p_qt, '9: ...and so does the weight they give');
END $$;

TRUNCATE entities, values, relations, spaces, user_votes,
         personalization.user_topic_signals, personalization.external_interest_signals,
         personalization.interest_sweep_state, personalization.interest_refit_runs CASCADE;
