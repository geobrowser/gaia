-- Assertions for 0101: account weights (GEO-3141).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
-- Voters, all scored at a fixed as_of so the new-account ramp is deterministic:
--   u_test     excluded by the analytics sync                   -> 0, reason excluded
--   u_new      first vote 1 day before as_of                    -> 1/7, reason new_account
--   u_old      30 days old, balanced stance votes               -> 1, no reasons
--   u_agree    30 days old, 25 of 25 stance votes Agree         -> 0.25, reason one_sided
--   u_disagree 30 days old, 25 of 25 stance votes Disagree      -> 0.25, reason one_sided
--   u_few      30 days old, 10 of 10 Agree (under the minimum)  -> 1
--   u_96       30 days old, 24 of 25 Agree (96% > 95%)          -> 0.25
--   u_95       30 days old, 19 of 20 Agree (exactly 95%, not >) -> 1
--   u_curate   30 days old, 30 curation upvotes, no stance votes -> 1 (curation is not stance)
--   u_manual   excluded manually; must survive an analytics re-sync

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

TRUNCATE public.user_votes, public.account_weights, public.account_exclusions CASCADE;

CREATE TEMP TABLE ids AS SELECT * FROM (VALUES
  ('u_test',     '01010000-0000-4000-8000-000000000001'::uuid),
  ('u_new',      '01010000-0000-4000-8000-000000000002'::uuid),
  ('u_old',      '01010000-0000-4000-8000-000000000003'::uuid),
  ('u_agree',    '01010000-0000-4000-8000-000000000004'::uuid),
  ('u_disagree', '01010000-0000-4000-8000-000000000005'::uuid),
  ('u_few',      '01010000-0000-4000-8000-000000000006'::uuid),
  ('u_96',       '01010000-0000-4000-8000-000000000007'::uuid),
  ('u_95',       '01010000-0000-4000-8000-000000000008'::uuid),
  ('u_curate',   '01010000-0000-4000-8000-000000000009'::uuid),
  ('u_manual',   '01010000-0000-4000-8000-00000000000a'::uuid),
  ('space',      '01010000-0000-4000-8000-0000000000a1'::uuid)
) AS v(key, id);
CREATE OR REPLACE FUNCTION pg_temp.id(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM ids WHERE key = k $$;

-- `n` votes by `who` of one kind and direction, on distinct objects, `days_ago` before as_of.
CREATE OR REPLACE FUNCTION pg_temp.votes(who text, n integer, kind smallint, vtype smallint, days_ago integer, salt integer)
RETURNS void LANGUAGE sql AS $$
  INSERT INTO public.user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at)
  SELECT pg_temp.id(who),
         md5(who || ':' || salt || ':' || g)::uuid,
         0, pg_temp.id('space'), vtype, kind,
         '2026-10-05 12:00:00+00'::timestamptz - make_interval(days => days_ago)
  FROM generate_series(1, n) g
$$;

SELECT pg_temp.votes('u_test', 5, 1::smallint, 0::smallint, 30, 1);
SELECT pg_temp.votes('u_new', 3, 1::smallint, 0::smallint, 1, 1);
SELECT pg_temp.votes('u_old', 10, 1::smallint, 0::smallint, 30, 1);
SELECT pg_temp.votes('u_old', 10, 1::smallint, 1::smallint, 30, 2);
SELECT pg_temp.votes('u_agree', 25, 1::smallint, 0::smallint, 30, 1);
SELECT pg_temp.votes('u_disagree', 25, 1::smallint, 1::smallint, 30, 1);
SELECT pg_temp.votes('u_few', 10, 1::smallint, 0::smallint, 30, 1);
SELECT pg_temp.votes('u_96', 24, 1::smallint, 0::smallint, 30, 1);
SELECT pg_temp.votes('u_96', 1, 1::smallint, 1::smallint, 30, 2);
SELECT pg_temp.votes('u_95', 19, 1::smallint, 0::smallint, 30, 1);
SELECT pg_temp.votes('u_95', 1, 1::smallint, 1::smallint, 30, 2);
SELECT pg_temp.votes('u_curate', 30, 0::smallint, 0::smallint, 30, 1);
SELECT pg_temp.votes('u_manual', 5, 1::smallint, 0::smallint, 30, 1);

-- Exclusions: one manual, then the analytics sync.
INSERT INTO public.account_exclusions (user_id, reason, source) VALUES (pg_temp.id('u_manual'), 'manual: QA account', 'manual');

DO $$ BEGIN
  PERFORM public.replace_account_exclusions('[]'::jsonb);
  RAISE EXCEPTION 'empty sync was accepted';
EXCEPTION WHEN invalid_parameter_value THEN RAISE NOTICE 'pass: an empty sync is refused';
END $$;

SELECT assert(public.replace_account_exclusions(jsonb_build_array(
  jsonb_build_object('user_id', pg_temp.id('u_test'), 'reason', 'test account: +test email'),
  jsonb_build_object('user_id', pg_temp.id('u_test'), 'reason', 'duplicate row in the sync'),
  -- also listed manually: the manual row wins and is not overwritten
  jsonb_build_object('user_id', pg_temp.id('u_manual'), 'reason', 'labelled test')
)) = 1, 'sync writes one row per account and leaves the manual exclusion alone');
SELECT assert((SELECT source FROM account_exclusions WHERE user_id = pg_temp.id('u_manual')) = 'manual',
  'manual exclusion keeps its source');

DO $$ BEGIN
  PERFORM public.refresh_account_weights(ramp_days => 0);
  RAISE EXCEPTION 'invalid parameters were accepted';
EXCEPTION WHEN invalid_parameter_value THEN RAISE NOTICE 'pass: invalid parameters are refused';
END $$;

SELECT assert(public.refresh_account_weights(as_of => '2026-10-05 12:00:00+00') = 10, 'every voter scored');

CREATE OR REPLACE FUNCTION pg_temp.w(who text) RETURNS double precision LANGUAGE sql AS $$
  SELECT weight FROM public.account_weights WHERE user_id = pg_temp.id(who) $$;
CREATE OR REPLACE FUNCTION pg_temp.codes(who text) RETURNS text LANGUAGE sql AS $$
  SELECT coalesce(string_agg(r ->> 'code', ',' ORDER BY r ->> 'code'), '')
  FROM public.account_weights aw, jsonb_array_elements(aw.reasons) r WHERE aw.user_id = pg_temp.id(who) $$;

SELECT assert(pg_temp.w('u_test') = 0 AND pg_temp.codes('u_test') = 'excluded', 'excluded test account weighs 0, with the reason');
SELECT assert((SELECT reasons -> 0 ->> 'detail' FROM account_weights WHERE user_id = pg_temp.id('u_test')) = 'test account: +test email',
  'exclusion reason is carried through');
SELECT assert(pg_temp.w('u_manual') = 0, 'manually excluded account weighs 0');
SELECT assert(abs(pg_temp.w('u_new') - 1.0 / 7) < 1e-9 AND pg_temp.codes('u_new') = 'new_account', 'a day-old account weighs 1/7');
SELECT assert(pg_temp.w('u_old') = 1 AND pg_temp.codes('u_old') = '', 'an established, balanced voter weighs 1 with no reasons');
SELECT assert(pg_temp.w('u_agree') = 0.25 AND pg_temp.codes('u_agree') = 'one_sided', 'always-Agree voter is weighted down');
SELECT assert(pg_temp.w('u_disagree') = 0.25 AND pg_temp.codes('u_disagree') = 'one_sided', 'always-Disagree voter is weighted down');
SELECT assert(pg_temp.w('u_few') = 1, 'one-sided but under 20 stance votes keeps full weight');
SELECT assert(pg_temp.w('u_96') = 0.25, '96% one way over 25 votes is one-sided');
SELECT assert(pg_temp.w('u_95') = 1, 'exactly 95% is not more than 95%');
SELECT assert(pg_temp.w('u_curate') = 1, 'curation upvotes are not stance votes');
SELECT assert((SELECT stance_vote_count FROM account_weights WHERE user_id = pg_temp.id('u_curate')) = 0,
  'stance vote count excludes curation votes');

SELECT assert(public.account_vote_weight(pg_temp.id('u_agree')) = 0.25, 'account_vote_weight reads the stored weight');
SELECT assert(public.account_vote_weight('01010000-0000-4000-8000-0000000000ff'::uuid) = 0, 'an unscored account counts 0');

-- Re-running is stable, and the ramp follows as_of.
SELECT assert(public.refresh_account_weights(as_of => '2026-10-05 12:00:00+00') = 10, 're-run scores the same voters');
SELECT assert((SELECT count(*) FROM account_weights) = 10, 're-run adds no rows');
SELECT public.refresh_account_weights(as_of => '2026-10-12 12:00:00+00');
SELECT assert(pg_temp.w('u_new') = 1 AND pg_temp.codes('u_new') = '', 'a week later the new account has full weight');

-- An account whose votes are gone loses its row.
DELETE FROM user_votes WHERE user_id = pg_temp.id('u_few');
SELECT public.refresh_account_weights(as_of => '2026-10-12 12:00:00+00');
SELECT assert(NOT EXISTS (SELECT 1 FROM account_weights WHERE user_id = pg_temp.id('u_few')), 'no votes, no weight row');

TRUNCATE public.user_votes, public.account_weights, public.account_exclusions CASCADE;
