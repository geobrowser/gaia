-- Assertions for 0098: New order, "all of these topics", and the composition counts (GEO-3092).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
-- Topics T1, T2. Newest first: a (claim, T1+T2), b (news, T1), c (claim tagged Debate, T1+T2),
-- d (news, T2). c's score is raised far above the rest, so Best and New disagree about it.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

TRUNCATE entities, values, relations, votes_count, entity_ranking_scores,
         entity_type_weights, entity_type_exclusions, entity_type_ranking,
         entity_topic_ranking, entity_feed_blocklist CASCADE;

UPDATE entity_ranking_config
   SET participation_weight = 4, participation_cap = 19,
       comment_weight = 0, comment_cap = 30, tau_seconds = 100000
 WHERE id;

CREATE TEMP TABLE f AS SELECT * FROM (VALUES
  ('a', '98000000-0000-4000-8000-00000000000a'::uuid, 1790500000),
  ('b', '98000000-0000-4000-8000-00000000000b'::uuid, 1790413600),
  ('c', '98000000-0000-4000-8000-00000000000c'::uuid, 1790327200),
  ('d', '98000000-0000-4000-8000-00000000000d'::uuid, 1790240800)
) AS v(k, id, created);
CREATE OR REPLACE FUNCTION pg_temp.e(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM f WHERE f.k = $1 $$;

CREATE TEMP TABLE ids AS SELECT * FROM (VALUES
  ('T1',    '98000000-0000-4000-8000-000000000101'::uuid),
  ('T2',    '98000000-0000-4000-8000-000000000102'::uuid),
  ('CLAIM', '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid),
  ('NEWS',  'e550fe51-7e90-4b2c-8fff-df13408f5634'::uuid),
  ('DEBATE_TAG', '55c95b26-26f8-482c-b973-9ea99dfde438'::uuid),
  ('S',     'bbbbbbbb-0000-0000-0000-000000000001'::uuid)
) AS v(k, id);
CREATE OR REPLACE FUNCTION pg_temp.c(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM ids WHERE ids.k = $1 $$;

INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
SELECT id, created::text, '0', created::text, '0' FROM f;

INSERT INTO values (id, entity_id, property_id, space_id, text)
SELECT gen_random_uuid()::text, id, 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid, pg_temp.c('S'), 'fixture ' || k
FROM f;

CREATE OR REPLACE FUNCTION pg_temp.rel(from_id uuid, type_id uuid, to_id uuid) RETURNS void
LANGUAGE sql AS $$
  INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
  VALUES (gen_random_uuid(), gen_random_uuid(), type_id, from_id, to_id,
          'bbbbbbbb-0000-0000-0000-000000000001'::uuid, false)
$$;

DO $$
DECLARE
  topics constant uuid := '806d52bc-27e9-4c91-93c0-57978b093351';
  types  constant uuid := '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1';
  tags   constant uuid := '25709034-1ba5-406f-94e4-d4af90042fba';
BEGIN
  PERFORM pg_temp.rel(pg_temp.e(k), types, pg_temp.c('CLAIM')) FROM f WHERE k IN ('a','c');
  PERFORM pg_temp.rel(pg_temp.e(k), types, pg_temp.c('NEWS')) FROM f WHERE k IN ('b','d');
  PERFORM pg_temp.rel(pg_temp.e(k), topics, pg_temp.c('T1')) FROM f WHERE k IN ('a','b','c');
  PERFORM pg_temp.rel(pg_temp.e(k), topics, pg_temp.c('T2')) FROM f WHERE k IN ('a','c','d');
  PERFORM pg_temp.rel(pg_temp.e('c'), tags, pg_temp.c('DEBATE_TAG'));
END $$;

SELECT public.refresh_entity_ranking_scores(ARRAY(SELECT id FROM f));
-- Lift c far above the rest, then let the reconcile carry it into the topic rows.
UPDATE entity_ranking_scores SET ranking_score = ranking_score + 100 WHERE entity_id = pg_temp.e('c');
SELECT public.reconcile_entity_topic_ranking();

CREATE OR REPLACE FUNCTION pg_temp.feed(topic_keys text[], type_keys text[], sort text, all_of boolean, rule boolean)
RETURNS text LANGUAGE sql AS $$
  SELECT coalesce(string_agg(f.k, ',' ORDER BY o.n), '')
  FROM public.entities_ranked_for_topics(
         ARRAY(SELECT pg_temp.c(k) FROM unnest(topic_keys) k), NULL,
         CASE WHEN type_keys IS NULL THEN NULL ELSE ARRAY(SELECT pg_temp.c(k) FROM unnest(type_keys) k) END,
         NULL, NULL, NULL, NULL, 100, rule, sort, all_of)
       WITH ORDINALITY AS o(id, created_at, created_at_block, updated_at, updated_at_block, n)
  JOIN f ON f.id = o.id
$$;

-- ---------------------------------------------------------------------------
-- 1. The rows carry creation time.
-- ---------------------------------------------------------------------------
SELECT assert((SELECT count(*) FROM entity_topic_ranking WHERE created_epoch IS NULL) = 0,
  'every row has created_epoch after the reconcile');

-- ---------------------------------------------------------------------------
-- 2. Best puts the high-scoring c first; New puts it where its age says.
-- ---------------------------------------------------------------------------
DO $$
DECLARE got text;
BEGIN
  got := pg_temp.feed(ARRAY['T1','T2'], ARRAY['CLAIM','NEWS'], 'best', false, false);
  PERFORM assert(got = 'c,a,b,d', format('Best: c first on its score (got %s)', got));
  got := pg_temp.feed(ARRAY['T1','T2'], ARRAY['CLAIM','NEWS'], 'new', false, false);
  PERFORM assert(got = 'a,b,c,d', format('New: newest first, whatever the score (got %s)', got));
  got := pg_temp.feed(ARRAY['T1','T2'], NULL, 'new', false, false);
  PERFORM assert(got = 'a,b,c,d', format('New without types gives the same (got %s)', got));
END $$;

-- ---------------------------------------------------------------------------
-- 3. "All of these topics": only a and c carry both.
-- ---------------------------------------------------------------------------
DO $$
DECLARE got text;
BEGIN
  got := pg_temp.feed(ARRAY['T1','T2'], ARRAY['CLAIM','NEWS'], 'best', true, false);
  PERFORM assert(got = 'c,a', format('all-of, Best (got %s)', got));
  got := pg_temp.feed(ARRAY['T1','T2'], ARRAY['CLAIM','NEWS'], 'new', true, false);
  PERFORM assert(got = 'a,c', format('all-of, New (got %s)', got));
  got := pg_temp.feed(ARRAY['T2','T1'], NULL, 'best', true, false);
  PERFORM assert(got = 'c,a', format('all-of does not depend on topic order, untyped (got %s)', got));
  got := pg_temp.feed(ARRAY['T1','T2'], ARRAY['CLAIM','NEWS'], 'best', true, true);
  PERFORM assert(got = 'c', format('all-of with the Debate-tag rule: only the tagged claim (got %s)', got));
END $$;

-- ---------------------------------------------------------------------------
-- 4. The composition counts.
-- ---------------------------------------------------------------------------
DO $$
DECLARE got text;
BEGIN
  SELECT string_agg(i.k || ':' || n.entity_count, ' ' ORDER BY i.k) INTO got
  FROM public.topic_feed_type_counts(ARRAY[pg_temp.c('T1')], ARRAY[pg_temp.c('CLAIM'), pg_temp.c('NEWS')]) n
  JOIN ids i ON i.id = n.type_id;
  PERFORM assert(got = 'CLAIM:2 NEWS:1', format('T1 holds two claims and one news story (got %s)', got));

  SELECT string_agg(i.k || ':' || n.entity_count, ' ' ORDER BY i.k) INTO got
  FROM public.topic_feed_type_counts(ARRAY[pg_temp.c('T1'), pg_temp.c('T2')], ARRAY[pg_temp.c('CLAIM'), pg_temp.c('NEWS')]) n
  JOIN ids i ON i.id = n.type_id;
  PERFORM assert(got = 'CLAIM:2', format('narrowed to T1 and T2: the two claims, no news (got %s)', got));

  SELECT string_agg(i.k || ':' || n.entity_count, ' ' ORDER BY i.k) INTO got
  FROM public.topic_feed_type_counts(ARRAY[pg_temp.c('T1')], ARRAY[pg_temp.c('CLAIM'), pg_temp.c('NEWS')],
                                     NULL, true, true) n
  JOIN ids i ON i.id = n.type_id;
  PERFORM assert(got = 'CLAIM:1 NEWS:1', format('with the Debate-tag rule only the tagged claim counts (got %s)', got));
END $$;

-- ---------------------------------------------------------------------------
-- 5. A sort that is not best or new is refused as bad input.
-- ---------------------------------------------------------------------------
DO $$
BEGIN
  BEGIN
    PERFORM public.entities_ranked_for_topics(ARRAY[pg_temp.c('T1')], NULL, NULL, NULL, NULL, NULL, NULL, 10, false, 'top');
    PERFORM assert(false, 'an unknown sort must be refused');
  EXCEPTION WHEN invalid_parameter_value THEN
    PERFORM assert(true, 'an unknown sort is refused as bad input');
  END;
END $$;
