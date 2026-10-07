-- Assertions for 0108: the stance map in shadow (GEO-3146).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
-- Observations (stance_map_observations):
--   u_a    weight 1    Agree on c1, then Disagree on c1 later (latest wins); Agree on c2;
--                      a curation upvote on c3 (not stance); Agree on n1 (not a Claim)
--   u_b    weight 0.5  Disagree on c2
--   u_zero weight 0    Agree on c1 (excluded account: no row at all)
--   u_none no weight   Agree on c1 (not yet scored: no row)
--   debates: d1 on c1 (u_a Supported by, u_b Opposed by), d2 a topic debate (no Claims: states
--            nothing), d3 names two main claims (ambiguous: skipped), d4 and d5 on c2 with u_b on
--            opposite sides (a tie: dropped), d6 on c2 with u_zero (weight 0: dropped)
-- Seeds (stance_map_seeds): c2 Supports c1, c3 Opposes c1, c4 both Supports and Opposes c1
--   (contradictory: dropped), c5 Addresses c1 (no direction: not a seed).
-- Runs (record_stance_map_run): the gate, its failures, and that a run row holds no user id.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

TRUNCATE public.user_votes, public.account_weights, public.relations, public.entities,
         personalization.stance_map_runs CASCADE;
UPDATE personalization.stance_map_config
   SET min_clean_votes = 10000, min_split_correlation = 0.7, min_auc_lift = 0.0, stated_weight = 1.0;

CREATE TEMP TABLE ids AS SELECT * FROM (VALUES
  ('u_a',    '01080000-0000-4000-8000-000000000001'::uuid),
  ('u_b',    '01080000-0000-4000-8000-000000000002'::uuid),
  ('u_zero', '01080000-0000-4000-8000-000000000003'::uuid),
  ('u_none', '01080000-0000-4000-8000-000000000004'::uuid),
  ('c1',     '01080000-0000-4000-8000-0000000000c1'::uuid),
  ('c2',     '01080000-0000-4000-8000-0000000000c2'::uuid),
  ('c3',     '01080000-0000-4000-8000-0000000000c3'::uuid),
  ('c4',     '01080000-0000-4000-8000-0000000000c4'::uuid),
  ('c5',     '01080000-0000-4000-8000-0000000000c5'::uuid),
  ('n1',     '01080000-0000-4000-8000-0000000000e1'::uuid),
  ('d1',     '01080000-0000-4000-8000-0000000000d1'::uuid),
  ('d2',     '01080000-0000-4000-8000-0000000000d2'::uuid),
  ('d3',     '01080000-0000-4000-8000-0000000000d3'::uuid),
  ('d4',     '01080000-0000-4000-8000-0000000000d4'::uuid),
  ('d5',     '01080000-0000-4000-8000-0000000000d5'::uuid),
  ('d6',     '01080000-0000-4000-8000-0000000000d6'::uuid),
  ('t1',     '01080000-0000-4000-8000-0000000000f1'::uuid),
  ('space',  '01080000-0000-4000-8000-0000000000a1'::uuid)
) AS v(key, id);
CREATE OR REPLACE FUNCTION pg_temp.id(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM ids WHERE key = k $$;

CREATE OR REPLACE FUNCTION pg_temp.rel(type_id uuid, from_key text, to_key text) RETURNS void
LANGUAGE sql AS $$
  INSERT INTO public.relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id)
  VALUES (gen_random_uuid(), gen_random_uuid(), type_id, pg_temp.id(from_key), pg_temp.id(to_key), pg_temp.id('space'))
$$;

INSERT INTO public.entities (id, created_at, created_at_block, updated_at, updated_at_block)
SELECT id, '1790000000', '0', '1790000000', '0' FROM ids WHERE key ~ '^(c|n|d|t)[0-9]$';

-- Types: c1..c5 are Claims, n1 is not; d1..d6 are Debates.
INSERT INTO public.relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id)
SELECT gen_random_uuid(), gen_random_uuid(), '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1',
       i.id, CASE WHEN i.key LIKE 'c%' THEN '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid
                  ELSE 'fd51f935-2063-4617-be39-7b672b23364c'::uuid END,
       pg_temp.id('space')
FROM ids i WHERE i.key ~ '^(c|d)[0-9]$';

-- Debates. Claims = e614cce1, Supported by = d19fad56, Opposed by = c57de77c, Topics = 806d52bc.
SELECT pg_temp.rel('e614cce1-c4ce-4586-8304-fd1237119eb2', 'd1', 'c1');
SELECT pg_temp.rel('d19fad56-5136-4a7f-8309-daf5c7bf99dd', 'd1', 'u_a');
SELECT pg_temp.rel('c57de77c-3eee-4e7b-a0d2-258d18aab11c', 'd1', 'u_b');
SELECT pg_temp.rel('806d52bc-27e9-4c91-93c0-57978b093351', 'd2', 't1');
SELECT pg_temp.rel('d19fad56-5136-4a7f-8309-daf5c7bf99dd', 'd2', 'u_a');
SELECT pg_temp.rel('e614cce1-c4ce-4586-8304-fd1237119eb2', 'd3', 'c1');
SELECT pg_temp.rel('e614cce1-c4ce-4586-8304-fd1237119eb2', 'd3', 'c2');
SELECT pg_temp.rel('d19fad56-5136-4a7f-8309-daf5c7bf99dd', 'd3', 'u_a');
SELECT pg_temp.rel('e614cce1-c4ce-4586-8304-fd1237119eb2', 'd4', 'c2');
SELECT pg_temp.rel('d19fad56-5136-4a7f-8309-daf5c7bf99dd', 'd4', 'u_b');
SELECT pg_temp.rel('e614cce1-c4ce-4586-8304-fd1237119eb2', 'd5', 'c2');
SELECT pg_temp.rel('c57de77c-3eee-4e7b-a0d2-258d18aab11c', 'd5', 'u_b');
SELECT pg_temp.rel('e614cce1-c4ce-4586-8304-fd1237119eb2', 'd6', 'c2');
SELECT pg_temp.rel('d19fad56-5136-4a7f-8309-daf5c7bf99dd', 'd6', 'u_zero');

-- Seeds. Supports = 81faa4ad, Opposes = 71d1bcd5, Addresses = 7115a43d.
SELECT pg_temp.rel('81faa4ad-afad-4009-b106-1374c1219f04', 'c2', 'c1');
SELECT pg_temp.rel('71d1bcd5-f1cf-487f-b9a3-c7b1d53803eb', 'c3', 'c1');
SELECT pg_temp.rel('81faa4ad-afad-4009-b106-1374c1219f04', 'c4', 'c1');
SELECT pg_temp.rel('71d1bcd5-f1cf-487f-b9a3-c7b1d53803eb', 'c4', 'c1');
SELECT pg_temp.rel('7115a43d-cac3-47b8-b036-aa113c6ad1b0', 'c5', 'c1');

INSERT INTO public.account_weights (user_id, weight, reasons, vote_count, stance_vote_count, first_vote_at, computed_at)
SELECT pg_temp.id(k), w, '[]'::jsonb, 1, 1, now() - interval '30 days', now()
FROM (VALUES ('u_a', 1.0), ('u_b', 0.5), ('u_zero', 0.0)) v(k, w);

INSERT INTO public.user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at)
SELECT pg_temp.id(u), pg_temp.id(o), 0, pg_temp.id('space'), vt::smallint, vk::smallint, now() - make_interval(days => ago)
FROM (VALUES
  ('u_a', 'c1', 0, 1, 10),
  ('u_b', 'c2', 1, 1, 5),
  ('u_a', 'c2', 0, 1, 5),
  ('u_a', 'c3', 0, 0, 5),
  ('u_a', 'n1', 0, 1, 5),
  ('u_zero', 'c1', 0, 1, 5),
  ('u_none', 'c1', 0, 1, 5)
) v(u, o, vt, vk, ago);
-- u_a changes their mind on c1, in another space: the latest stance wins.
INSERT INTO public.user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at)
VALUES (pg_temp.id('u_a'), pg_temp.id('c1'), 0, gen_random_uuid(), 1, 1, now() - interval '1 day');

CREATE TEMP TABLE o AS
SELECT i_u.key AS u, i_c.key AS c, x.agree, x.weight, x.source
FROM personalization.stance_map_observations() x
JOIN ids i_u ON i_u.id = x.user_id
JOIN ids i_c ON i_c.id = x.claim_id;

SELECT assert((SELECT count(*) FROM o) = 5, 'five observations: 3 votes, 2 stated sides');
SELECT assert((SELECT count(*) FROM personalization.stance_map_observations()) = (SELECT count(*) FROM o),
              'every observation is a fixture user and claim');
SELECT assert((SELECT NOT agree FROM o WHERE u = 'u_a' AND c = 'c1' AND source = 'vote'), 'latest stance vote wins');
SELECT assert((SELECT agree AND weight = 1.0 FROM o WHERE u = 'u_a' AND c = 'c2' AND source = 'vote'), 'u_a agrees with c2');
SELECT assert((SELECT NOT agree AND weight = 0.5 FROM o WHERE u = 'u_b' AND c = 'c2' AND source = 'vote'),
              'votes carry the account weight');
SELECT assert(NOT EXISTS (SELECT 1 FROM o WHERE c IN ('c3', 'n1')), 'curation votes and non-claims are not stances');
SELECT assert(NOT EXISTS (SELECT 1 FROM o WHERE u IN ('u_zero', 'u_none')), 'weight-0 and unscored accounts are left out');
SELECT assert((SELECT agree FROM o WHERE u = 'u_a' AND c = 'c1' AND source = 'stated'), 'Supported by is a stated Agree');
SELECT assert((SELECT NOT agree AND weight = 0.5 FROM o WHERE u = 'u_b' AND c = 'c1' AND source = 'stated'),
              'Opposed by is a stated Disagree, at the account weight');
SELECT assert((SELECT count(*) FROM o WHERE source = 'stated') = 2,
              'topic debates, ambiguous debates and tied sides state nothing');

UPDATE personalization.stance_map_config SET stated_weight = 0.25;
SELECT assert((SELECT weight FROM personalization.stance_map_observations() x
               WHERE x.user_id = pg_temp.id('u_b') AND x.source = 'stated') = 0.125,
              'stated_weight scales a stated side');
UPDATE personalization.stance_map_config SET stated_weight = 0;
SELECT assert(NOT EXISTS (SELECT 1 FROM personalization.stance_map_observations() x WHERE x.source = 'stated'),
              'stated_weight 0 turns stated sides off');
UPDATE personalization.stance_map_config SET stated_weight = 1.0;

-- Seeds.
CREATE TEMP TABLE s AS
SELECT i_c.key AS c, i_m.key AS m, x.sign
FROM personalization.stance_map_seeds() x
JOIN ids i_c ON i_c.id = x.claim_id JOIN ids i_m ON i_m.id = x.main_claim_id;
SELECT assert((SELECT count(*) FROM s) = 2, 'two seeds: contradictory and Addresses pairs are not seeds');
SELECT assert((SELECT sign FROM s WHERE c = 'c2' AND m = 'c1') = 1, 'Supports is +1');
SELECT assert((SELECT sign FROM s WHERE c = 'c3' AND m = 'c1') = -1, 'Opposes is -1');

-- Runs and the gate.
DO $$ BEGIN
  PERFORM personalization.record_stance_map_run('{"clean_votes": 5}'::jsonb);
  RAISE EXCEPTION 'a run without model_version was accepted';
EXCEPTION WHEN invalid_parameter_value THEN RAISE NOTICE 'pass: a malformed run is refused';
END $$;

SELECT assert(personalization.record_stance_map_run(
  '{"model_version": "t", "clean_votes": 4474, "voters": 128, "auc_map": 0.70, "auc_tendencies": 0.675,
    "auc_lift": 0.025, "split_correlation": 0.15, "split_claims": 80, "detail": {"note": "2 Oct"}}'::jsonb) = false,
  'the 2 Oct numbers do not clear the gate');
SELECT assert((SELECT gate_failures = ARRAY['clean_votes', 'split_correlation'] FROM personalization.stance_map_runs
               ORDER BY id DESC LIMIT 1), 'the failures name volume and stability');
SELECT assert((SELECT detail ->> 'note' = '2 Oct' AND voters = 128 FROM personalization.stance_map_runs
               ORDER BY id DESC LIMIT 1), 'metrics and detail are stored');

SELECT assert(personalization.record_stance_map_run(
  '{"model_version": "t", "clean_votes": 12000, "auc_lift": 0.04, "split_correlation": 0.75}'::jsonb),
  'enough votes, stable and better than tendencies clears the gate');
SELECT assert((SELECT gate_met AND gate_failures = '{}' FROM personalization.stance_map_runs ORDER BY id DESC LIMIT 1),
              'a met gate has no failures');

SELECT assert(NOT personalization.record_stance_map_run(
  '{"model_version": "t", "clean_votes": 12000, "auc_lift": -0.01, "split_correlation": 0.9}'::jsonb),
  'a stable map that loses to tendencies does not clear the gate');
SELECT assert(NOT personalization.record_stance_map_run(
  '{"model_version": "t", "clean_votes": 12000, "auc_lift": 0.05}'::jsonb),
  'an unmeasured stability (NULL) fails its condition');
SELECT assert((SELECT gate_failures = ARRAY['split_correlation'] FROM personalization.stance_map_runs ORDER BY id DESC LIMIT 1),
              'NULL stability is reported as a failure');

UPDATE personalization.stance_map_config SET min_clean_votes = 4000, min_split_correlation = 0.1;
SELECT assert(personalization.record_stance_map_run(
  '{"model_version": "t", "clean_votes": 4474, "auc_lift": 0.025, "split_correlation": 0.15}'::jsonb),
  'the thresholds come from stance_map_config');
UPDATE personalization.stance_map_config SET min_clean_votes = 10000, min_split_correlation = 0.7;

SELECT assert((SELECT count(*) FROM personalization.stance_map_runs) = 5, 'every run is kept as history');

-- Privacy: the run table holds aggregates only, and nothing here is in `public`.
SELECT assert(NOT EXISTS (
  SELECT 1 FROM information_schema.columns
  WHERE table_schema = 'personalization' AND table_name LIKE 'stance_map%'
    AND (column_name LIKE '%user%' OR data_type = 'uuid')), 'no stance map table has a user or entity id column');
SELECT assert(NOT EXISTS (
  SELECT 1 FROM information_schema.tables WHERE table_schema = 'public' AND table_name LIKE 'stance_map%'),
  'no stance map table is in public');
SELECT assert(NOT EXISTS (
  SELECT 1 FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
  WHERE n.nspname = 'public' AND p.proname LIKE '%stance_map%'), 'no stance map function is in public');

TRUNCATE public.user_votes, public.account_weights, public.relations, public.entities,
         personalization.stance_map_runs CASCADE;
