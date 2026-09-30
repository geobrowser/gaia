-- Assertions for 0096: the For you feed over followed topics (GEO-3077).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
-- The risks, each asserted below:
--   1. Rows that can never be shown get in: unnamed, System or editorially excluded entities.
--      The single largest live topic is 32,704 unnamed entities, so this is a latency bug too.
--   2. An entity comes back more than once: once per matching topic, or, untyped, once per type.
--   3. The order is not Best's when no weights are given, or weights do not reorder.
--   4. Paging with max_per_topic = offset + first loses or repeats an entity.
--   5. The hook or the reconcile fails to add a new tag, or to drop a removed one.

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

-- ---------------------------------------------------------------------------
-- Fixtures. Two topics (T1, T2), two feed types (Claim C, News N), one excluded type (X).
-- Created a day apart so newer ranks higher; `a` is newest.
--   a  C      T1           shown
--   b  N      T1, T2       shown once, whichever topic
--   c  C, N   T1           two types: shown once, untyped and typed
--   d  C      T2           shown
--   e  C      T1           oldest; used to test weights
--   u  C      T1           UNNAMED: never shown
--   s  C      T1           a System entity: never shown
--   x  C, X   T1           an excluded type: never shown
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE f AS SELECT * FROM (VALUES
  ('a', '96000000-0000-4000-8000-00000000000a'::uuid, 1790500000),
  ('b', '96000000-0000-4000-8000-00000000000b'::uuid, 1790413600),
  ('c', '96000000-0000-4000-8000-00000000000c'::uuid, 1790327200),
  ('d', '96000000-0000-4000-8000-00000000000d'::uuid, 1790240800),
  ('e', '96000000-0000-4000-8000-00000000000e'::uuid, 1790154400),
  ('u', '96000000-0000-4000-8000-0000000000a1'::uuid, 1790600000),
  ('s', '96000000-0000-4000-8000-0000000000a2'::uuid, 1790600000),
  ('x', '96000000-0000-4000-8000-0000000000a3'::uuid, 1790600000)
) AS v(k, id, created);
CREATE OR REPLACE FUNCTION pg_temp.e(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM f WHERE f.k = $1 $$;

-- Topic, type and system ids.
CREATE TEMP TABLE c AS SELECT * FROM (VALUES
  ('T1', '96000000-0000-4000-8000-000000000101'::uuid),
  ('T2', '96000000-0000-4000-8000-000000000102'::uuid),
  ('C',  '96000000-0000-4000-8000-000000000201'::uuid),
  ('N',  '96000000-0000-4000-8000-000000000202'::uuid),
  ('X',  '96000000-0000-4000-8000-000000000203'::uuid)
) AS v(k, id);
CREATE OR REPLACE FUNCTION pg_temp.c(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM c WHERE c.k = $1 $$;

INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
SELECT id, created::text, '0', created::text, '0' FROM f;

INSERT INTO values (id, entity_id, property_id, space_id, text)
SELECT gen_random_uuid()::text, id, 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid,
       'bbbbbbbb-0000-0000-0000-000000000001'::uuid, 'fixture ' || k
FROM f WHERE k <> 'u';

INSERT INTO entity_type_exclusions (type_id, note) VALUES (pg_temp.c('X'), '0096 fixture');

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
  sys    constant uuid := '88b3d6ad-288c-529c-a212-0e1c24819185';
BEGIN
  PERFORM pg_temp.rel(pg_temp.e(k), types, pg_temp.c('C')) FROM f WHERE k IN ('a','c','d','e','u','s','x');
  PERFORM pg_temp.rel(pg_temp.e(k), types, pg_temp.c('N')) FROM f WHERE k IN ('b','c');
  PERFORM pg_temp.rel(pg_temp.e('x'), types, pg_temp.c('X'));
  PERFORM pg_temp.rel(pg_temp.e('s'), sys, pg_temp.c('C'));
  PERFORM pg_temp.rel(pg_temp.e(k), topics, pg_temp.c('T1')) FROM f WHERE k IN ('a','b','c','e','u','s','x');
  PERFORM pg_temp.rel(pg_temp.e(k), topics, pg_temp.c('T2')) FROM f WHERE k IN ('b','d');
END $$;

-- Scores exist through the real choke point; its hook fills the topic rows as it goes.
SELECT public.refresh_entity_ranking_scores(ARRAY(SELECT id FROM f));

-- A feed as a comma-separated list of fixture keys, for readable assertions.
CREATE OR REPLACE FUNCTION pg_temp.keys(ids uuid[]) RETURNS text LANGUAGE sql AS $$
  SELECT string_agg(f.k, ',' ORDER BY o.n) FROM unnest(ids) WITH ORDINALITY AS o(id, n) JOIN f ON f.id = o.id
$$;
CREATE OR REPLACE FUNCTION pg_temp.feed(topic_keys text[], weights float8[], type_keys text[], cap int) RETURNS text
LANGUAGE sql AS $$
  SELECT pg_temp.keys(ARRAY(
    SELECT r.id FROM public.entities_ranked_for_topics(
      ARRAY(SELECT pg_temp.c(k) FROM unnest(topic_keys) k),
      weights,
      CASE WHEN type_keys IS NULL THEN NULL ELSE ARRAY(SELECT pg_temp.c(k) FROM unnest(type_keys) k) END,
      NULL, NULL, NULL, NULL, cap) r))
$$;

-- ---------------------------------------------------------------------------
-- 1. Only showable entities get rows, one per (topic, type) they carry.
-- ---------------------------------------------------------------------------
DO $$
DECLARE got text;
BEGIN
  SELECT string_agg(f.k || ':' || tc.k || ':' || yc.k, ' ' ORDER BY f.k, tc.k, yc.k) INTO got
  FROM entity_topic_ranking r
  JOIN f ON f.id = r.entity_id JOIN c tc ON tc.id = r.topic_id JOIN c yc ON yc.id = r.type_id;
  PERFORM assert(got = 'a:T1:C b:T1:N b:T2:N c:T1:C c:T1:N d:T2:C e:T1:C',
    format('the hook wrote rows for a-e only, none for the unnamed, System or excluded entity (got %s)', got));
END $$;

-- ---------------------------------------------------------------------------
-- 2 and 3. Best's order, each entity once, typed or not.
-- ---------------------------------------------------------------------------
DO $$
DECLARE got text;
BEGIN
  got := pg_temp.feed(ARRAY['T1','T2'], NULL, ARRAY['C','N'], 100);
  PERFORM assert(got = 'a,b,c,d,e', format('typed: newest first, b and c once each (got %s)', got));
  got := pg_temp.feed(ARRAY['T1','T2'], NULL, NULL, 100);
  PERFORM assert(got = 'a,b,c,d,e', format('untyped gives the same feed (got %s)', got));
  got := pg_temp.feed(ARRAY['T1','T2'], NULL, ARRAY['N'], 100);
  PERFORM assert(got = 'b,c', format('only the requested type (got %s)', got));
  got := pg_temp.feed(ARRAY['T1','T2'], ARRAY[1.5, 1.5], ARRAY['C','N'], 100);
  PERFORM assert(got = 'a,b,c,d,e', format('equal weights leave Best''s order alone (got %s)', got));
  -- Four days of recency is 3.456 score units; a 5-unit weight on T2 lifts d (T2, three days
  -- older than a) above everything tagged only T1. b is on T2 too, so it rises with it.
  got := pg_temp.feed(ARRAY['T1','T2'], ARRAY[0, 5], ARRAY['C','N'], 100);
  PERFORM assert(got = 'b,d,a,c,e', format('a weight ranks its topic higher (got %s)', got));
END $$;

-- ---------------------------------------------------------------------------
-- 4. Paging: each page asks for max_per_topic = offset + first, and the pages together are the
--    whole feed exactly once. The untyped cap must count entities, not rows: c has two rows
--    in T1, so a cap of 4 counting rows would stop at a, b, c, c and return three.
-- ---------------------------------------------------------------------------
DO $$
DECLARE whole text; paged text := ''; page text; o int;
BEGIN
  whole := pg_temp.feed(ARRAY['T1','T2'], NULL, NULL, 100);
  FOR o IN 0..4 BY 2 LOOP
    SELECT string_agg(k, ',' ORDER BY n) INTO page FROM (
      SELECT f.k, row_number() OVER () AS n
      FROM public.entities_ranked_for_topics(
             ARRAY[pg_temp.c('T1'), pg_temp.c('T2')], NULL, NULL, NULL, NULL, NULL, NULL, o + 2) r
      JOIN f ON f.id = r.id
      OFFSET o LIMIT 2) p;
    paged := paged || CASE WHEN paged = '' THEN '' ELSE ',' END || coalesce(page, '');
  END LOOP;
  PERFORM assert(paged = whole, format('three pages of two are the whole feed once (%s vs %s)', paged, whole));
  PERFORM assert(pg_temp.feed(ARRAY['T1'], NULL, NULL, 4) = 'a,b,c,e',
    format('an untyped cap of 4 reaches four entities though c has two rows (got %s)', pg_temp.feed(ARRAY['T1'], NULL, NULL, 4)));
END $$;

-- ---------------------------------------------------------------------------
-- The matched topics, and the weights guard.
-- ---------------------------------------------------------------------------
DO $$
DECLARE got uuid[];
BEGIN
  SELECT public.entities_matched_topic_ids(e, ARRAY[pg_temp.c('T1'), pg_temp.c('T2')]) INTO got
  FROM entities e WHERE e.id = pg_temp.e('b');
  PERFORM assert(got = ARRAY[pg_temp.c('T1'), pg_temp.c('T2')], 'b reports both followed topics it matched');
  BEGIN
    PERFORM public.entities_ranked_for_topics(ARRAY[pg_temp.c('T1'), pg_temp.c('T2')], ARRAY[1.0]::float8[]);
    PERFORM assert(false, 'a weights list of the wrong length must be refused');
  EXCEPTION WHEN invalid_parameter_value THEN
    PERFORM assert(true, 'a weights list of the wrong length is refused as bad input');
  END;
END $$;

-- ---------------------------------------------------------------------------
-- 5. Freshness. A tag added to an entity whose score does not change: the reconcile adds it.
--    A tag removed: the hook, on the next score change, drops it; so does the reconcile.
-- ---------------------------------------------------------------------------
DO $$
DECLARE n int;
BEGIN
  PERFORM pg_temp.rel(pg_temp.e('a'), '806d52bc-27e9-4c91-93c0-57978b093351', pg_temp.c('T2'));
  SELECT count(*) INTO n FROM entity_topic_ranking WHERE entity_id = pg_temp.e('a') AND topic_id = pg_temp.c('T2');
  PERFORM assert(n = 0, 'a new tag is not in the table before anything runs');
  PERFORM public.reconcile_entity_topic_ranking();
  SELECT count(*) INTO n FROM entity_topic_ranking WHERE entity_id = pg_temp.e('a') AND topic_id = pg_temp.c('T2');
  PERFORM assert(n = 1, 'the reconcile adds a tag no score change reported');

  DELETE FROM relations WHERE from_entity_id = pg_temp.e('d') AND type_id = '806d52bc-27e9-4c91-93c0-57978b093351';
  PERFORM public.refresh_entity_ranking_scores(ARRAY[pg_temp.e('d')]);
  SELECT count(*) INTO n FROM entity_topic_ranking WHERE entity_id = pg_temp.e('d');
  PERFORM assert(n = 0, 'the hook drops a removed tag for the ids it scores');

  -- A name removed without a score change: the reconcile drops the entity.
  DELETE FROM values WHERE entity_id = pg_temp.e('e');
  PERFORM public.reconcile_entity_topic_ranking();
  SELECT count(*) INTO n FROM entity_topic_ranking WHERE entity_id = pg_temp.e('e');
  PERFORM assert(n = 0, 'the reconcile drops an entity that lost its name');

  SELECT public.reconcile_entity_topic_ranking() INTO n;
  PERFORM assert(n = 0, format('a second reconcile changes nothing (changed %s)', n));
END $$;
