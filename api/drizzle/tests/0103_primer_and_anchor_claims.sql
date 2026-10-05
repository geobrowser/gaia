-- Assertions for 0103: primer and anchor claims (GEO-3143).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
-- Spaces: S1 (a user's top pick), S2 (second pick), S3 (picked, holds no claims, own topic T4),
-- SX (not picked). Voters v1..v40 weigh 1, junk voters j1..j30 weigh 0 (GEO-3141). A claim with
-- a agrees and d disagrees is voted Agree by v1..va and Disagree by v(a+1)..v(a+d).
--
--   claim     spaces  topic  agree/disagree     note
--   hot1      S1      T1      8 / 7             + 10 curation voters: the most ENGAGING contested in S1
--   hot2      S1      T1     10 / 10            higher score than hot1, less engagement
--   s1_t2     S1      T2      6 / 6
--   s2_a      S2      T3      5 / 5
--   pop       S1      T6     20 / 0             popular and unanimous: little information
--   junk      S1      T6      2 / 0 (+30 junk)  30 weight-0 Disagrees must not make it contested
--   a1        SX      T4     12 / 12
--   a2        SX      T5     11 / 11
--   a3        SX      T4      9 / 9
--   unnamed   S1      T1     10 / 10            no name: never a candidate
--   notclaim  S1      T1     10 / 10            not typed Claim: never a candidate
--   longname  S2      T3      4 / 4             name of 300 characters

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

TRUNCATE public.user_votes, public.account_weights, public.account_exclusions,
         public.primer_claim_stats, public.anchor_claim_sets, public.anchor_claim_members,
         public.claim_overlap_samples, public.relations, public.values, public.spaces, public.entities CASCADE;

CREATE TEMP TABLE ids AS SELECT * FROM (VALUES
  ('s1',       '01030000-0000-4000-8000-0000000000a1'::uuid),
  ('s2',       '01030000-0000-4000-8000-0000000000a2'::uuid),
  ('s3',       '01030000-0000-4000-8000-0000000000a3'::uuid),
  ('sx',       '01030000-0000-4000-8000-0000000000a9'::uuid),
  ('t1',       '01030000-0000-4000-8000-000000000101'::uuid),
  ('t2',       '01030000-0000-4000-8000-000000000102'::uuid),
  ('t3',       '01030000-0000-4000-8000-000000000103'::uuid),
  ('t4',       '01030000-0000-4000-8000-000000000104'::uuid),
  ('t5',       '01030000-0000-4000-8000-000000000105'::uuid),
  ('t6',       '01030000-0000-4000-8000-000000000106'::uuid),
  ('hot1',     '01030000-0000-4000-8000-000000000201'::uuid),
  ('hot2',     '01030000-0000-4000-8000-000000000202'::uuid),
  ('s1_t2',    '01030000-0000-4000-8000-000000000203'::uuid),
  ('s2_a',     '01030000-0000-4000-8000-000000000204'::uuid),
  ('pop',      '01030000-0000-4000-8000-000000000205'::uuid),
  ('junk',     '01030000-0000-4000-8000-000000000206'::uuid),
  ('a1',       '01030000-0000-4000-8000-000000000207'::uuid),
  ('a2',       '01030000-0000-4000-8000-000000000208'::uuid),
  ('a3',       '01030000-0000-4000-8000-000000000209'::uuid),
  ('unnamed',  '01030000-0000-4000-8000-00000000020a'::uuid),
  ('notclaim', '01030000-0000-4000-8000-00000000020b'::uuid),
  ('longname', '01030000-0000-4000-8000-00000000020c'::uuid),
  ('user_u',   '01030000-0000-4000-8000-000000000301'::uuid),
  ('user_v',   '01030000-0000-4000-8000-000000000302'::uuid)
) AS v(key, id);
CREATE OR REPLACE FUNCTION pg_temp.id(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM ids WHERE key = k $$;
CREATE OR REPLACE FUNCTION pg_temp.voter(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT md5('0103-voter-' || k)::uuid $$;

INSERT INTO public.entities (id, created_at, created_at_block, updated_at, updated_at_block)
SELECT id, '1790000000', '0', '1790000000', '0' FROM ids WHERE key LIKE 't_';

INSERT INTO public.spaces (id, type, address, topic_id) VALUES
  (pg_temp.id('s1'), 'DAO', '0x0103a1', NULL),
  (pg_temp.id('s2'), 'DAO', '0x0103a2', NULL),
  (pg_temp.id('s3'), 'DAO', '0x0103a3', pg_temp.id('t4')),
  (pg_temp.id('sx'), 'DAO', '0x0103a9', NULL);

-- Real voters weigh 1, junk voters 0, as refresh_account_weights would leave them.
INSERT INTO public.account_weights (user_id, weight, reasons, vote_count, stance_vote_count, first_vote_at, computed_at)
SELECT pg_temp.voter('v' || g), 1.0, '[]'::jsonb, 1, 1, now() - interval '60 days', now() FROM generate_series(1, 40) g
UNION ALL
SELECT pg_temp.voter('j' || g), 0.0, '[{"code":"excluded"}]'::jsonb, 1, 1, now() - interval '60 days', now() FROM generate_series(1, 30) g;

-- A claim: an entity typed Claim in each space, tagged with a topic, named, and voted on.
CREATE OR REPLACE FUNCTION pg_temp.claim(k text, spaces text[], topic text, agrees int, disagrees int,
                                         named boolean DEFAULT true, typed boolean DEFAULT true,
                                         label text DEFAULT NULL)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE
  sp text;
BEGIN
  INSERT INTO public.entities (id, created_at, created_at_block, updated_at, updated_at_block)
  VALUES (pg_temp.id(k), '1790000000', '0', '1790000000', '0');
  FOREACH sp IN ARRAY spaces LOOP
    IF typed THEN
      INSERT INTO public.relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id)
      VALUES (gen_random_uuid(), gen_random_uuid(), '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1',
              pg_temp.id(k), '96f859ef-a1ca-4b22-9372-c86ad58b694b', pg_temp.id(sp));
    END IF;
  END LOOP;
  INSERT INTO public.relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id)
  VALUES (gen_random_uuid(), gen_random_uuid(), '806d52bc-27e9-4c91-93c0-57978b093351',
          pg_temp.id(k), pg_temp.id(topic), pg_temp.id(spaces[1]));
  IF named THEN
    INSERT INTO public.values (id, property_id, entity_id, space_id, text)
    VALUES (gen_random_uuid()::text, 'a126ca53-0c8e-48d5-b888-82c734c38935', pg_temp.id(k),
            pg_temp.id(spaces[1]), coalesce(label, 'Claim ' || k));
  END IF;
  INSERT INTO public.user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at)
  SELECT pg_temp.voter('v' || g), pg_temp.id(k), 0, pg_temp.id(spaces[1]),
         CASE WHEN g <= agrees THEN 0 ELSE 1 END, 1, now() - interval '10 days'
  FROM generate_series(1, agrees + disagrees) g;
END $$;

SELECT pg_temp.claim('hot1', ARRAY['s1'], 't1', 8, 7);
SELECT pg_temp.claim('hot2', ARRAY['s1'], 't1', 10, 10);
SELECT pg_temp.claim('s1_t2', ARRAY['s1'], 't2', 6, 6);
SELECT pg_temp.claim('s2_a', ARRAY['s2'], 't3', 5, 5);
SELECT pg_temp.claim('pop', ARRAY['s1'], 't6', 20, 0);
SELECT pg_temp.claim('junk', ARRAY['s1'], 't6', 2, 0);
SELECT pg_temp.claim('a1', ARRAY['sx'], 't4', 12, 12);
SELECT pg_temp.claim('a2', ARRAY['sx'], 't5', 11, 11);
SELECT pg_temp.claim('a3', ARRAY['sx'], 't4', 9, 9);
SELECT pg_temp.claim('unnamed', ARRAY['s1'], 't1', 10, 10, named => false);
SELECT pg_temp.claim('notclaim', ARRAY['s1'], 't1', 10, 10, typed => false);
SELECT pg_temp.claim('longname', ARRAY['s2'], 't3', 4, 4, label => repeat('x', 300));

-- hot1's engagement: ten curation voters on top of its fifteen stance voters.
INSERT INTO public.user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at)
SELECT pg_temp.voter('v' || g), pg_temp.id('hot1'), 0, pg_temp.id('s1'), 0, 0, now() - interval '10 days'
FROM generate_series(25, 34) g;
-- junk's thirty weight-0 Disagrees.
INSERT INTO public.user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at)
SELECT pg_temp.voter('j' || g), pg_temp.id('junk'), 0, pg_temp.id('s1'), 1, 1, now() - interval '10 days'
FROM generate_series(1, 30) g;
-- user_v already took a stance on hot1 (outside the primer).
INSERT INTO public.user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at)
VALUES (pg_temp.id('user_v'), pg_temp.id('hot1'), 0, pg_temp.id('s1'), 0, 1, now() - interval '1 day');

----------------------------------------------------------------------------------------------
-- Statistics
----------------------------------------------------------------------------------------------

DO $$ BEGIN
  PERFORM public.refresh_primer_claim_stats(min_voters => 3, contested_low => 0.8, contested_high => 0.2);
  RAISE EXCEPTION 'inverted contested band was accepted';
EXCEPTION WHEN invalid_parameter_value THEN RAISE NOTICE 'pass: an inverted contested band is refused';
END $$;

SELECT assert(public.refresh_primer_claim_stats() = 10, 'ten claims scored (unnamed and notclaim are not)');
SELECT assert(NOT EXISTS (SELECT 1 FROM public.primer_claim_stats WHERE claim_id IN (pg_temp.id('unnamed'), pg_temp.id('notclaim'))),
              'an unnamed claim and an untyped entity are never candidates');

SELECT assert((SELECT voters = 32 AND weighted_voters = 2 AND agree_share = 1 AND NOT contested
               FROM public.primer_claim_stats WHERE claim_id = pg_temp.id('junk')),
              'weight-0 voters count in voters but not in weighted_voters, so junk is not contested');
SELECT assert((SELECT contested FROM public.primer_claim_stats WHERE claim_id = pg_temp.id('hot1')),
              '8/7 is contested');
SELECT assert((SELECT NOT contested AND information < 0.3 FROM public.primer_claim_stats WHERE claim_id = pg_temp.id('pop')),
              'a unanimous claim carries little information, however popular');
SELECT assert((SELECT score FROM public.primer_claim_stats WHERE claim_id = pg_temp.id('pop'))
              < (SELECT score FROM public.primer_claim_stats WHERE claim_id = pg_temp.id('s2_a')),
              'a 5/5 split with 10 voters outscores a 20/0 one with 20');
SELECT assert((SELECT score FROM public.primer_claim_stats WHERE claim_id = pg_temp.id('hot2'))
              > (SELECT score FROM public.primer_claim_stats WHERE claim_id = pg_temp.id('hot1')),
              'hot2 outscores hot1');
SELECT assert((SELECT engagement FROM public.primer_claim_stats WHERE claim_id = pg_temp.id('hot1'))
              > (SELECT engagement FROM public.primer_claim_stats WHERE claim_id = pg_temp.id('hot2')),
              'hot1 is more engaging than hot2 (curation voters count)');
SELECT assert((SELECT name_length FROM public.primer_claim_stats WHERE claim_id = pg_temp.id('longname')) = 300,
              'name length is recorded');

----------------------------------------------------------------------------------------------
-- Anchors: monthly, with history
----------------------------------------------------------------------------------------------

CREATE TEMP TABLE months AS
SELECT date_trunc('month', now()) - interval '1 month' + interval '1 day' AS last_month,
       date_trunc('month', now()) AS this_month;

-- Last month's set: 4 claims, at most one per topic. By score: a1 (T4), a2 (T5), hot2 (T1),
-- a3 (T4, capped), hot1 (T1, capped), s1_t2 (T2).
SELECT assert(public.refresh_anchor_claims(set_size => 4, min_size => 2, min_voters => 5, max_per_topic => 1,
                                           as_of => (SELECT last_month FROM months)) IS NOT NULL,
              'the first anchor set is created');
SELECT assert((SELECT array_agg(m.claim_id ORDER BY m.rank) FROM public.anchor_claim_members m
               JOIN public.anchor_claim_sets s ON s.id = m.set_id WHERE s.effective_from = (SELECT last_month FROM months))
              = ARRAY[pg_temp.id('a1'), pg_temp.id('a2'), pg_temp.id('hot2'), pg_temp.id('s1_t2')],
              'anchors are the best-scoring contested claims, one per topic');
SELECT assert(public.refresh_anchor_claims(as_of => (SELECT last_month + interval '1 day' FROM months)) IS NULL,
              'a second run in the same month does nothing');
SELECT assert((SELECT count(*) FROM public.anchor_claim_sets) = 1, 'still one set');

-- Without the topic cap, min_size fills from the uncapped contested list.
SELECT assert(public.refresh_anchor_claims(set_size => 3, min_size => 3, min_voters => 5, max_per_topic => 100,
                                           force => true, as_of => (SELECT last_month + interval '2 days' FROM months)) IS NOT NULL,
              'a forced run in the same month creates a set');
SELECT assert((SELECT array_agg(m.claim_id ORDER BY m.rank) FROM public.anchor_claim_members m
               WHERE m.set_id = public.anchor_claim_set_at((SELECT last_month + interval '2 days' FROM months)))
              = ARRAY[pg_temp.id('a1'), pg_temp.id('a2'), pg_temp.id('hot2')],
              'uncapped: the top three by score');
-- Never pick a non-contested or thinly voted claim while contested ones remain.
SELECT assert(NOT EXISTS (SELECT 1 FROM public.anchor_claim_members m JOIN public.primer_claim_stats s USING (claim_id)
                          WHERE NOT s.contested), 'no anchor is uncontested');

-- This month's set: just a1 and a2, both outside the spaces the test users pick.
SELECT assert(public.refresh_anchor_claims(set_size => 2, min_size => 2, min_voters => 5, max_per_topic => 1,
                                           as_of => (SELECT this_month FROM months)) IS NOT NULL,
              'a new month gets a new set');
SELECT assert((SELECT array_agg(id ORDER BY id) FROM public.anchor_claims())
              = (SELECT array_agg(x ORDER BY x) FROM unnest(ARRAY[pg_temp.id('a1'), pg_temp.id('a2')]) x),
              'anchor_claims() returns the set in force now');
SELECT assert((SELECT array_agg(id) FROM public.anchor_claims((SELECT last_month + interval '1 day' FROM months)))
              = ARRAY[pg_temp.id('a1'), pg_temp.id('a2'), pg_temp.id('hot2'), pg_temp.id('s1_t2')],
              'the anchors in force on a past date are recoverable, in rank order');
SELECT assert(NOT EXISTS (SELECT 1 FROM public.anchor_claims((SELECT last_month - interval '1 day' FROM months))),
              'before the first set there are no anchors');

DO $$ BEGIN
  PERFORM public.refresh_anchor_claims(force => true, as_of => (SELECT last_month FROM months));
  RAISE EXCEPTION 'a set older than the newest was accepted';
EXCEPTION WHEN invalid_parameter_value THEN RAISE NOTICE 'pass: history cannot be rewritten';
END $$;

----------------------------------------------------------------------------------------------
-- Primer: user_u picks S1 then S2 and answers in order
----------------------------------------------------------------------------------------------

CREATE OR REPLACE FUNCTION pg_temp.next(who text, picked text[], answered text[], skipped text[] DEFAULT '{}')
RETURNS uuid LANGUAGE sql AS $$
  SELECT id FROM public.next_primer_claim(
    pg_temp.id(who),
    ARRAY(SELECT pg_temp.id(p) FROM unnest(picked) WITH ORDINALITY a(p, o) ORDER BY o),
    ARRAY(SELECT pg_temp.id(p) FROM unnest(answered) p),
    ARRAY(SELECT pg_temp.id(p) FROM unnest(skipped) p))
$$;

SELECT assert(pg_temp.next('user_u', ARRAY['s1','s2'], '{}') = pg_temp.id('hot1'),
              'claim 1: the most engaging contested claim in the top space (hot1, not the higher-scoring hot2)');
SELECT assert(pg_temp.next('user_u', ARRAY['s1','s2'], ARRAY['hot1']) = pg_temp.id('s1_t2'),
              'claim 2: a contested claim on a topic not yet answered (T2) beats hot2 on the answered T1');
SELECT assert(pg_temp.next('user_u', ARRAY['s1','s2'], ARRAY['hot1','s1_t2']) = pg_temp.id('s2_a'),
              'claim 3: the second picked space, while it still leaves room for two anchors');
SELECT assert(pg_temp.next('user_u', ARRAY['s1','s2'], ARRAY['hot1','s1_t2','s2_a']) = pg_temp.id('a1'),
              'claim 4: two slots left and two anchors needed, so an anchor');
SELECT assert((SELECT reason FROM public.primer_candidates(pg_temp.id('user_u'),
                 ARRAY[pg_temp.id('s1'), pg_temp.id('s2')],
                 ARRAY[pg_temp.id('hot1'), pg_temp.id('s1_t2'), pg_temp.id('s2_a')], '{}') WHERE rank = 1) = 'anchor needed',
              'claim 4 says why');
SELECT assert(pg_temp.next('user_u', ARRAY['s1','s2'], ARRAY['hot1','s1_t2','s2_a','a1']) = pg_temp.id('a2'),
              'claim 5: the second anchor');
SELECT assert(pg_temp.next('user_u', ARRAY['s1','s2'], ARRAY['hot1','s1_t2','s2_a','a1','a2']) IS NULL,
              'after five answers there is no next claim');

-- The rules hold however the primer goes: walk it greedily and check the five.
CREATE TEMP TABLE walk (n int, claim uuid);
DO $$
DECLARE
  answered uuid[] := '{}';
  nxt uuid;
BEGIN
  FOR i IN 1..6 LOOP
    SELECT id INTO nxt FROM public.next_primer_claim(pg_temp.id('user_u'), ARRAY[pg_temp.id('s1'), pg_temp.id('s2')], answered, NULL);
    EXIT WHEN nxt IS NULL;
    INSERT INTO walk VALUES (i, nxt);
    answered := answered || nxt;
  END LOOP;
END $$;
SELECT assert((SELECT count(*) FROM walk) = 5, 'a primer is exactly five claims');
SELECT assert((SELECT count(DISTINCT claim) FROM walk) = 5, 'no claim is asked twice');
SELECT assert((SELECT count(*) FROM walk w JOIN public.anchor_claim_members m ON m.claim_id = w.claim
               WHERE m.set_id = public.anchor_claim_set_at(now())) >= 2, 'at least two of the five are anchors');
SELECT assert((SELECT count(DISTINCT p) FROM walk w JOIN public.primer_claim_stats s ON s.claim_id = w.claim,
               unnest(s.space_ids) p WHERE p IN (pg_temp.id('s1'), pg_temp.id('s2'))) >= 2,
              'the five span both picked spaces');

-- Skipped and already-voted claims never come back.
SELECT assert(NOT EXISTS (SELECT 1 FROM public.primer_candidates(pg_temp.id('user_u'),
                 ARRAY[pg_temp.id('s1'), pg_temp.id('s2')], ARRAY[pg_temp.id('hot1')], ARRAY[pg_temp.id('s1_t2')])
                 WHERE claim_id IN (pg_temp.id('s1_t2'), pg_temp.id('hot1'))),
              'a skipped or answered claim is never a candidate');
SELECT assert(pg_temp.next('user_u', ARRAY['s1','s2'], ARRAY['hot1'], ARRAY['s1_t2']) = pg_temp.id('s2_a'),
              'after a skip the next best is offered');
SELECT assert(pg_temp.next('user_v', ARRAY['s1','s2'], '{}') = pg_temp.id('hot2'),
              'a claim the user already voted on is never offered: the opener falls to the next most engaging');
SELECT assert(pg_temp.next('user_v', ARRAY['s1','s2'], '{}', ARRAY['hot2']) = pg_temp.id('s1_t2'),
              'and so on down the top space');

-- Thin picks: S3 holds no claims, so anchors on its topic (T4) fill in first, and the user still
-- has at least five to answer.
SELECT assert(pg_temp.next('user_u', ARRAY['s3'], '{}') = pg_temp.id('a1'),
              'an empty picked space falls back to an anchor on a related topic');
SELECT assert((SELECT pool FROM public.primer_candidates(pg_temp.id('user_u'), ARRAY[pg_temp.id('s3')], '{}', '{}') WHERE rank = 1)
              = 'related_anchor', 'and says so');
SELECT assert((SELECT count(*) FROM public.primer_candidates(pg_temp.id('user_u'), ARRAY[pg_temp.id('s3')], '{}', '{}')) >= 5,
              'a user with thin picks still has at least five claims to answer');
SELECT assert(pg_temp.next('user_u', '{}', '{}') = pg_temp.id('a1'), 'no picks: anchors first');

-- A space with fewer than pool_min eligible claims widens the pool even when it is not empty:
-- S2 holds s2_a and longname.
SELECT assert(EXISTS (SELECT 1 FROM public.primer_candidates(pg_temp.id('user_u'), ARRAY[pg_temp.id('s2')], '{}', '{}')
                      WHERE pool = 'any'), 'a thin picked space widens the pool');
-- S1 + S2 hold six eligible claims: enough at pool_min 2, and enough for the five slots.
SELECT assert(NOT EXISTS (SELECT 1 FROM public.primer_candidates(pg_temp.id('user_u'), ARRAY[pg_temp.id('s1'), pg_temp.id('s2')], '{}', '{}', pool_min => 2)
                          WHERE pool = 'any'), 'picked spaces with enough claims do not');

-- Wording: max_name_chars drops long names when set.
SELECT assert(EXISTS (SELECT 1 FROM public.primer_candidates(pg_temp.id('user_u'), ARRAY[pg_temp.id('s2')], '{}', '{}')
                      WHERE claim_id = pg_temp.id('longname')), 'long names are allowed by default');
SELECT assert(NOT EXISTS (SELECT 1 FROM public.primer_candidates(pg_temp.id('user_u'), ARRAY[pg_temp.id('s2')], '{}', '{}', max_name_chars => 200)
                          WHERE claim_id = pg_temp.id('longname')), 'and dropped with max_name_chars');

----------------------------------------------------------------------------------------------
-- Overlap measure
----------------------------------------------------------------------------------------------

-- Brute force: every pair of counted users, and the claims they share.
CREATE TEMP TABLE brute AS
WITH s AS (
  SELECT DISTINCT uv.user_id, uv.object_id FROM public.user_votes uv
  JOIN public.account_weights w ON w.user_id = uv.user_id AND w.weight > 0
  WHERE uv.vote_kind = 1 AND uv.object_id IN (SELECT from_entity_id FROM public.relations
                                               WHERE to_entity_id = '96f859ef-a1ca-4b22-9372-c86ad58b694b')
), u AS (SELECT user_id FROM s GROUP BY 1 HAVING count(*) >= 5)
SELECT count(*) AS pairs,
       count(*) FILTER (WHERE (SELECT count(*) FROM s x JOIN s y ON x.object_id = y.object_id
                               WHERE x.user_id = a.user_id AND y.user_id = b.user_id) >= 3) AS sharing
FROM u a JOIN u b ON a.user_id < b.user_id;

SELECT assert((SELECT o.pairs = b.pairs AND o.pairs_sharing = b.sharing AND o.pairs > 0
               FROM public.claim_overlap_share(5, 3) o, brute b),
              'claim_overlap_share matches a brute-force pair count');
SELECT assert(public.record_claim_overlap_sample(), 'the first sample of the day is recorded');
SELECT assert(NOT public.record_claim_overlap_sample(), 'a second sample the same day is not');
SELECT assert((SELECT anchor_set_id = public.anchor_claim_set_at(now()) FROM public.claim_overlap_samples),
              'a sample records the anchor set in force');

TRUNCATE public.user_votes, public.account_weights, public.primer_claim_stats, public.anchor_claim_sets,
         public.anchor_claim_members, public.claim_overlap_samples, public.relations, public.values,
         public.spaces, public.entities CASCADE;
