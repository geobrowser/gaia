-- Assertions for 0091: the feed-composition canary actually detects a composition change.
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
-- Removes only its own fixtures, so it passes in any order and is safe against a database
-- holding real rows.
--
-- The thing worth testing is not "does it insert a row" — every implementation inserts a
-- row, including the ones that would have missed #948. The assertion that matters is the
-- last one: reproduce the #948 regression in miniature and prove the canary reports it.
-- Everything above that exists to make that assertion trustworthy.

\set ON_ERROR_STOP on

-- `IS NOT TRUE`, not `NOT cond`, and the difference is not pedantic. Under `NOT cond` a
-- NULL condition is neither true nor false, so it takes the ELSE branch and reports a
-- PASS. That is the vacuous-assertion trap drizzle/tests/README.md describes, and the
-- first draft of this very file hit it: a `SELECT ... INTO` that matched no row left the
-- variable NULL and the assertion below it printed "pass" with an empty value in the
-- message. Every other suite in this directory still uses the loose form; tightening them
-- is a follow-up worth doing, and likely to surface more stale fixtures.
CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

DELETE FROM entity_feed_composition_samples WHERE type_ids @> ARRAY['91000000-0000-4000-8000-0000000000aa'::uuid];
DELETE FROM entity_type_ranking   WHERE entity_id::text LIKE '00000091-%';
DELETE FROM entity_ranking_scores WHERE entity_id::text LIKE '00000091-%';
DELETE FROM votes_count           WHERE object_id::text LIKE '00000091-%';
DELETE FROM relations             WHERE from_entity_id::text LIKE '00000091-%';
DELETE FROM values                WHERE entity_id::text LIKE '00000091-%';
DELETE FROM entities              WHERE id::text        LIKE '00000091-%';

UPDATE entity_ranking_config
   SET participation_weight = 7, participation_cap = 30,
       comment_weight = 0, comment_cap = 30, tau_seconds = 100000
 WHERE id;

-- ---------------------------------------------------------------------------
-- Fixtures: the two halves of the #948 regression, in miniature.
--
--   TYPE_A (…aa) "the type that earns votes"  — 3 entities, 10 days old, 8 curation votes
--   TYPE_B (…bb) "the numerous recent type"   — 3 entities, brand new, no votes
--
-- At participation_weight 7 the voted older entities outrank the newer ones
-- (7*ln(9) = 15.38, plus ~1.8 of Wilson quality, against 10 days = 8.64 units).
-- At weight 0 they cannot (1.8 < 8.64) and the window flips to TYPE_B entirely.
-- That flip is #948, and the last assertion is that the canary sees it.
-- ---------------------------------------------------------------------------
INSERT INTO entities (id, created_at) VALUES
  ('00000091-0000-4000-8000-00000000000a', (extract(epoch from now())::bigint - 864000)::text),
  ('00000091-0000-4000-8000-00000000000b', (extract(epoch from now())::bigint - 864000)::text),
  ('00000091-0000-4000-8000-00000000000c', (extract(epoch from now())::bigint - 864000)::text),
  ('00000091-0000-4000-8000-00000000001a', extract(epoch from now())::bigint::text),
  ('00000091-0000-4000-8000-00000000001b', extract(epoch from now())::bigint::text),
  ('00000091-0000-4000-8000-00000000001c', extract(epoch from now())::bigint::text);

-- Candidate generation drops unnamed entities (0075).
INSERT INTO values (id, entity_id, property_id, space_id, text)
SELECT gen_random_uuid()::text, e.id, 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid,
       '91000000-0000-4000-8000-0000000000ff'::uuid, 'canary fixture'
  FROM entities e WHERE e.id::text LIKE '00000091-%';

-- TYPES relations are what entity_type_ranking reconciles from (0084).
INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
SELECT gen_random_uuid(), gen_random_uuid(),
       '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid,
       e.id,
       CASE WHEN e.id::text LIKE '%00000000000%' THEN '91000000-0000-4000-8000-0000000000aa'::uuid
            ELSE '91000000-0000-4000-8000-0000000000bb'::uuid END,
       '91000000-0000-4000-8000-0000000000ff'::uuid, false
  FROM entities e WHERE e.id::text LIKE '00000091-%';

INSERT INTO votes_count (object_id, object_type, space_id, vote_kind, positive, negative)
SELECT e.id, 0, '91000000-0000-4000-8000-0000000000ff'::uuid, 0, 8, 0
  FROM entities e WHERE e.id::text LIKE '00000091-0000-4000-8000-00000000000%';

SELECT assert(public.refresh_entity_ranking_scores(ARRAY(
  SELECT id FROM entities WHERE id::text LIKE '00000091-%')) = 6,
  'fixtures scored');

-- ---------------------------------------------------------------------------
-- 1. Guards. A sampler that accepts nonsense records nonsense forever.
-- ---------------------------------------------------------------------------
DO $$
BEGIN
  BEGIN
    PERFORM public.sample_feed_composition(ARRAY[]::uuid[], 66, NULL);
    PERFORM assert(false, 'an empty type list must be rejected, not sampled');
  EXCEPTION WHEN raise_exception THEN
    PERFORM assert(true, 'an empty type list is rejected');
  END;
  BEGIN
    PERFORM public.sample_feed_composition(ARRAY['91000000-0000-4000-8000-0000000000aa'::uuid], 0, NULL);
    PERFORM assert(false, 'a zero window must be rejected, not sampled');
  EXCEPTION WHEN raise_exception THEN
    PERFORM assert(true, 'a zero window is rejected');
  END;
END $$;

-- ---------------------------------------------------------------------------
-- 2. The window is a window. Counting every candidate instead of the top N is the
--    mistake that would make every sample look identical forever.
-- ---------------------------------------------------------------------------
DO $$
DECLARE sid bigint; total int; w int;
BEGIN
  sid := public.sample_feed_composition(
    ARRAY['91000000-0000-4000-8000-0000000000aa'::uuid,
          '91000000-0000-4000-8000-0000000000bb'::uuid], 2, NULL);
  SELECT window_size INTO w FROM entity_feed_composition_samples WHERE id = sid;
  SELECT sum(value::int) INTO total
    FROM entity_feed_composition_samples s, jsonb_each_text(s.composition) WHERE s.id = sid;
  PERFORM assert(w = 2, format('window_size is recorded as asked (%s)', w));
  PERFORM assert(total = 2,
    format('a window of 2 counts 2 rows, not every candidate (got %s of 6)', total));
END $$;

-- ---------------------------------------------------------------------------
-- 3. The config snapshot is the record that did not exist before #948.
-- ---------------------------------------------------------------------------
DO $$
DECLARE sid bigint; w numeric; c numeric; t numeric;
BEGIN
  UPDATE entity_ranking_config SET participation_weight = 3.25, participation_cap = 17 WHERE id;
  sid := public.sample_feed_composition(ARRAY['91000000-0000-4000-8000-0000000000aa'::uuid], 66, NULL);
  SELECT participation_weight, participation_cap, tau_seconds INTO w, c, t
    FROM entity_feed_composition_samples WHERE id = sid;
  PERFORM assert(w = 3.25 AND c = 17 AND t = 100000,
    format('the sample records the config in force when it was taken (%s / %s / %s)', w, c, t));
  UPDATE entity_ranking_config SET participation_weight = 7, participation_cap = 30 WHERE id;
END $$;

-- ---------------------------------------------------------------------------
-- 4. Ages. The median is the other half of the signal — #948 was caught on
--    composition, but its predecessor complaint was entirely about age.
-- ---------------------------------------------------------------------------
DO $$
DECLARE sid bigint; med bigint; oldest bigint;
BEGIN
  sid := public.sample_feed_composition(ARRAY['91000000-0000-4000-8000-0000000000aa'::uuid], 66, NULL);
  SELECT median_age_seconds, oldest_age_seconds INTO med, oldest
    FROM entity_feed_composition_samples WHERE id = sid;
  PERFORM assert(med BETWEEN 864000 - 120 AND 864000 + 120,
    format('median age of three 10-day-old entities is ~864000s (got %s)', med));
  PERFORM assert(oldest >= med, format('oldest >= median (%s >= %s)', oldest, med));
END $$;

-- A non-numeric created_at must not cost the whole sample.
DO $$
DECLARE sid bigint; med bigint;
BEGIN
  UPDATE entities SET created_at = 'not-a-number'
   WHERE id = '00000091-0000-4000-8000-00000000000a';
  sid := public.sample_feed_composition(ARRAY['91000000-0000-4000-8000-0000000000aa'::uuid], 66, NULL);
  SELECT median_age_seconds INTO med FROM entity_feed_composition_samples WHERE id = sid;
  PERFORM assert(med IS NOT NULL, 'an unparseable created_at is skipped, not fatal');
  UPDATE entities SET created_at = (extract(epoch from now())::bigint - 864000)::text
   WHERE id = '00000091-0000-4000-8000-00000000000a';
END $$;

-- ---------------------------------------------------------------------------
-- 5. Drift with a single sample is not an alarm.
-- ---------------------------------------------------------------------------
DELETE FROM entity_feed_composition_samples
 WHERE type_ids @> ARRAY['91000000-0000-4000-8000-0000000000aa'::uuid];

DO $$
DECLARE n int;
BEGIN
  PERFORM public.sample_feed_composition(
    ARRAY['91000000-0000-4000-8000-0000000000aa'::uuid,
          '91000000-0000-4000-8000-0000000000bb'::uuid], 3, NULL);
  SELECT count(*) INTO n FROM public.feed_composition_drift(NULL);
  PERFORM assert(n = 0, format('one sample reports no drift rather than a first-run alarm (got %s rows)', n));
END $$;

-- ---------------------------------------------------------------------------
-- 6. THE ASSERTION THIS FILE EXISTS FOR.
--
--    Reproduce #948: drop the participation weight, re-score, sample again. The
--    window flips from all TYPE_A to all TYPE_B, and the canary must report it.
--    If this passes, a change of that shape cannot ship unseen again.
-- ---------------------------------------------------------------------------
DO $$
DECLARE a_share numeric; b_share numeric; a_delta numeric; b_delta numeric; a_cur int; b_cur int;
BEGIN
  -- Before: TYPE_A owns the window at weight 7.
  --
  -- Read from the SAMPLE, not from feed_composition_drift(): only one sample exists here,
  -- so drift returns nothing by design and a `SELECT INTO` off it yields NULL. The first
  -- draft asserted on that NULL and passed vacuously.
  SELECT (composition ->> '91000000-0000-4000-8000-0000000000aa')::int INTO a_cur
    FROM entity_feed_composition_samples
   WHERE type_ids @> ARRAY['91000000-0000-4000-8000-0000000000aa'::uuid]
   ORDER BY id DESC LIMIT 1;
  PERFORM assert(a_cur = 3,
    format('precondition: at weight 7 the voted type owns the 3-row window (got %s of 3)', a_cur));

  UPDATE entity_ranking_config SET participation_weight = 0 WHERE id;
  PERFORM public.refresh_entity_ranking_scores(ARRAY(
    SELECT id FROM entities WHERE id::text LIKE '00000091-%'));
  PERFORM public.sample_feed_composition(
    ARRAY['91000000-0000-4000-8000-0000000000aa'::uuid,
          '91000000-0000-4000-8000-0000000000bb'::uuid], 3, NULL);

  SELECT share_delta, current_count INTO a_delta, a_cur FROM public.feed_composition_drift(NULL)
   WHERE type_id = '91000000-0000-4000-8000-0000000000aa';
  SELECT share_delta, current_count INTO b_delta, b_cur FROM public.feed_composition_drift(NULL)
   WHERE type_id = '91000000-0000-4000-8000-0000000000bb';

  PERFORM assert(a_delta IS NOT NULL AND b_delta IS NOT NULL,
    'drift returned a row for both types — a NULL here would pass every assertion below vacuously');
  PERFORM assert(a_cur = 0 AND b_cur = 3,
    format('the window flipped to the recent type (A=%s B=%s)', a_cur, b_cur));
  PERFORM assert(a_delta = -1.0,
    format('the canary reports the voted type losing the whole window (delta %s)', a_delta));
  PERFORM assert(b_delta = 1.0,
    format('the canary reports the recent type taking the whole window (delta %s)', b_delta));
END $$;

DELETE FROM entity_feed_composition_samples WHERE type_ids @> ARRAY['91000000-0000-4000-8000-0000000000aa'::uuid];
DELETE FROM entity_type_ranking   WHERE entity_id::text LIKE '00000091-%';
DELETE FROM entity_ranking_scores WHERE entity_id::text LIKE '00000091-%';
DELETE FROM votes_count           WHERE object_id::text LIKE '00000091-%';
DELETE FROM relations             WHERE from_entity_id::text LIKE '00000091-%';
DELETE FROM values                WHERE entity_id::text LIKE '00000091-%';
DELETE FROM entities              WHERE id::text        LIKE '00000091-%';

UPDATE entity_ranking_config
   SET participation_weight = 4.0, participation_cap = 19, tau_seconds = 100000
 WHERE id;

SELECT 'ALL 0091 ASSERTIONS PASSED' AS result;
