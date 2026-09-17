-- Assertions for 0093: window and rendered samples are separate histories.
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
-- The property that matters is isolation. A rendered sample and a window sample legitimately
-- differ — on 2026-09-17 the same feed was Claim 24 / Debate 8 / News 34 in the window and
-- 1.8 / 5.9 / 2.3 per 10 rendered — so comparing across sources would report a huge drift on
-- every single run and the alert would be worthless within a day.

\set ON_ERROR_STOP on

-- `IS NOT TRUE`, not `NOT cond`: a NULL condition would otherwise take the ELSE branch and
-- report a pass. See README.md.
CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

DELETE FROM entity_feed_composition_samples WHERE type_ids @> ARRAY['93000000-0000-4000-8000-0000000000aa'::uuid];

UPDATE entity_ranking_config
   SET participation_weight = 4.0, participation_cap = 19, tau_seconds = 100000
 WHERE id;

-- ---------------------------------------------------------------------------
-- 1. Guards. A source outside the two known values must not be storable, or the
--    drift function silently splits history into a third bucket nobody reads.
-- ---------------------------------------------------------------------------
DO $$
BEGIN
  BEGIN
    PERFORM public.record_feed_composition_sample(
      'nonsense', ARRAY['93000000-0000-4000-8000-0000000000aa'::uuid], '{}'::jsonb, 22);
    PERFORM assert(false, 'an unknown source must be rejected');
  EXCEPTION WHEN raise_exception THEN
    PERFORM assert(true, 'an unknown source is rejected');
  END;
  BEGIN
    PERFORM public.record_feed_composition_sample(
      'rendered', ARRAY[]::uuid[], '{}'::jsonb, 22);
    PERFORM assert(false, 'an empty type list must be rejected');
  EXCEPTION WHEN raise_exception THEN
    PERFORM assert(true, 'an empty type list is rejected');
  END;
END $$;

-- ---------------------------------------------------------------------------
-- 2. A rendered sample stores what it was given, and snapshots the live config —
--    the parameters are the half of a sample that did not exist before #948.
-- ---------------------------------------------------------------------------
DO $$
DECLARE sid bigint; src text; n int; w numeric; med bigint;
BEGIN
  sid := public.record_feed_composition_sample(
    'rendered',
    ARRAY['93000000-0000-4000-8000-0000000000aa'::uuid,'93000000-0000-4000-8000-0000000000bb'::uuid],
    '{"93000000-0000-4000-8000-0000000000aa": 13, "93000000-0000-4000-8000-0000000000bb": 9}'::jsonb,
    22, NULL, 86400, 604800);
  SELECT source, participation_weight, median_age_seconds INTO src, w, med
    FROM entity_feed_composition_samples WHERE id = sid;
  SELECT sum(value::int) INTO n
    FROM entity_feed_composition_samples s, jsonb_each_text(s.composition) WHERE s.id = sid;
  PERFORM assert(src = 'rendered', format('the sample is stored as rendered (got %s)', src));
  PERFORM assert(n = 22, format('the composition is stored verbatim (got %s of 22)', n));
  PERFORM assert(w = 4.0, format('the config snapshot is taken at write time (got %s)', w));
  PERFORM assert(med = 86400, format('the caller''s ages are stored (got %s)', med));
END $$;

-- ---------------------------------------------------------------------------
-- 3. THE ASSERTION THIS FILE EXISTS FOR.
--
--    Window and rendered are separate histories. Two samples of one source and one of
--    the other must yield drift for the first and none for the second — never a
--    cross-source comparison, which would fire on every run.
-- ---------------------------------------------------------------------------
DO $$
DECLARE n_rendered int; n_window int; d numeric;
BEGIN
  DELETE FROM entity_feed_composition_samples
   WHERE type_ids @> ARRAY['93000000-0000-4000-8000-0000000000aa'::uuid];

  -- Two rendered samples that differ, and a single window sample that differs from both.
  PERFORM public.record_feed_composition_sample('rendered',
    ARRAY['93000000-0000-4000-8000-0000000000aa'::uuid],
    '{"93000000-0000-4000-8000-0000000000aa": 10}'::jsonb, 22);
  PERFORM public.record_feed_composition_sample('window',
    ARRAY['93000000-0000-4000-8000-0000000000aa'::uuid],
    '{"93000000-0000-4000-8000-0000000000aa": 66}'::jsonb, 66);
  PERFORM public.record_feed_composition_sample('rendered',
    ARRAY['93000000-0000-4000-8000-0000000000aa'::uuid],
    '{"93000000-0000-4000-8000-0000000000aa": 4,
      "93000000-0000-4000-8000-0000000000bb": 18}'::jsonb, 22);

  SELECT count(*) INTO n_rendered FROM public.feed_composition_drift(NULL, 'rendered');
  SELECT count(*) INTO n_window   FROM public.feed_composition_drift(NULL, 'window');

  PERFORM assert(n_rendered = 2,
    format('rendered drift compares the two rendered samples (got %s rows)', n_rendered));
  PERFORM assert(n_window = 0,
    format('one window sample is not drift — the window history is its own (got %s rows)', n_window));

  SELECT share_delta INTO d FROM public.feed_composition_drift(NULL, 'rendered')
   WHERE type_id = '93000000-0000-4000-8000-0000000000aa';
  PERFORM assert(d IS NOT NULL, 'the drift row exists rather than passing vacuously');
  PERFORM assert(d < 0, format('the type that lost share reports a negative delta (got %s)', d));
END $$;

-- ---------------------------------------------------------------------------
-- 4. The default source keeps the pre-0093 behaviour, so nothing that called the
--    one-argument form starts silently reading a different history.
-- ---------------------------------------------------------------------------
DO $$
DECLARE n_default int; n_window int;
BEGIN
  SELECT count(*) INTO n_default FROM public.feed_composition_drift(NULL);
  SELECT count(*) INTO n_window  FROM public.feed_composition_drift(NULL, 'window');
  PERFORM assert(n_default = n_window,
    format('the default source is window (%s vs %s)', n_default, n_window));
END $$;

DELETE FROM entity_feed_composition_samples WHERE type_ids @> ARRAY['93000000-0000-4000-8000-0000000000aa'::uuid];

SELECT 'ALL 0093 ASSERTIONS PASSED' AS result;
