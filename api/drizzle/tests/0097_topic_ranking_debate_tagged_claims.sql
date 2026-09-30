-- Assertions for 0097: Debate-tagged claims as their own walk (GEO-3077 follow-up).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
-- One topic T. Newest first: c2 (claim, untagged), n1 (news), c1 (claim, tagged Debate from
-- space S). Every entity is named in both S and S2, so a space filter only ever changes the answer
-- through the tag's scoping.

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
  ('c2', '97000000-0000-4000-8000-0000000000c2'::uuid, 1790500000),
  ('n1', '97000000-0000-4000-8000-0000000000a1'::uuid, 1790413600),
  ('c1', '97000000-0000-4000-8000-0000000000c1'::uuid, 1790327200)
) AS v(k, id, created);
CREATE OR REPLACE FUNCTION pg_temp.e(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM f WHERE f.k = $1 $$;

-- Real ids: Explore's Claim and News story types and the Debate tag, since the function keys on them.
CREATE TEMP TABLE ids AS SELECT * FROM (VALUES
  ('T',     '97000000-0000-4000-8000-000000000101'::uuid),
  ('CLAIM', '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid),
  ('NEWS',  'e550fe51-7e90-4b2c-8fff-df13408f5634'::uuid),
  ('DEBATE_TAG', '55c95b26-26f8-482c-b973-9ea99dfde438'::uuid),
  ('S',     'bbbbbbbb-0000-0000-0000-000000000001'::uuid),
  ('S2',    'bbbbbbbb-0000-0000-0000-000000000002'::uuid)
) AS v(k, id);
CREATE OR REPLACE FUNCTION pg_temp.c(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM ids WHERE ids.k = $1 $$;

INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
SELECT id, created::text, '0', created::text, '0' FROM f;

INSERT INTO values (id, entity_id, property_id, space_id, text)
SELECT gen_random_uuid()::text, f.id, 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid, s.id, 'fixture ' || f.k
FROM f CROSS JOIN (SELECT pg_temp.c('S') AS id UNION ALL SELECT pg_temp.c('S2')) s;

CREATE OR REPLACE FUNCTION pg_temp.rel(from_id uuid, type_id uuid, to_id uuid, space uuid) RETURNS void
LANGUAGE sql AS $$
  INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
  VALUES (gen_random_uuid(), gen_random_uuid(), type_id, from_id, to_id, space, false)
$$;

DO $$
DECLARE
  topics constant uuid := '806d52bc-27e9-4c91-93c0-57978b093351';
  types  constant uuid := '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1';
  tags   constant uuid := '25709034-1ba5-406f-94e4-d4af90042fba';
BEGIN
  PERFORM pg_temp.rel(pg_temp.e('c1'), types, pg_temp.c('CLAIM'), pg_temp.c('S'));
  PERFORM pg_temp.rel(pg_temp.e('c2'), types, pg_temp.c('CLAIM'), pg_temp.c('S'));
  PERFORM pg_temp.rel(pg_temp.e('n1'), types, pg_temp.c('NEWS'), pg_temp.c('S'));
  PERFORM pg_temp.rel(pg_temp.e(k), topics, pg_temp.c('T'), pg_temp.c('S')) FROM f;
  PERFORM pg_temp.rel(pg_temp.e('c1'), tags, pg_temp.c('DEBATE_TAG'), pg_temp.c('S'));
END $$;

SELECT public.refresh_entity_ranking_scores(ARRAY(SELECT id FROM f));

CREATE OR REPLACE FUNCTION pg_temp.feed(type_keys text[], space_keys text[], cap int, rule boolean) RETURNS text
LANGUAGE sql AS $$
  SELECT coalesce(string_agg(f.k, ',' ORDER BY o.n), '')
  FROM public.entities_ranked_for_topics(
         ARRAY[pg_temp.c('T')], NULL,
         CASE WHEN type_keys IS NULL THEN NULL ELSE ARRAY(SELECT pg_temp.c(k) FROM unnest(type_keys) k) END,
         NULL, NULL, NULL,
         CASE WHEN space_keys IS NULL THEN NULL ELSE ARRAY(SELECT pg_temp.c(k) FROM unnest(space_keys) k) END,
         cap, rule) WITH ORDINALITY AS o(id, created_at, created_at_block, updated_at, updated_at_block, n)
  JOIN f ON f.id = o.id
$$;

-- ---------------------------------------------------------------------------
-- 1. The tagged claim has its own row; the untagged one does not.
-- ---------------------------------------------------------------------------
DO $$
DECLARE got text;
BEGIN
  SELECT string_agg(f.k, ',' ORDER BY f.k) INTO got
  FROM entity_topic_ranking r JOIN f ON f.id = r.entity_id WHERE r.type_id = pg_temp.c('DEBATE_TAG');
  PERFORM assert(got = 'c1', format('only c1 gets a Debate-tag row (got %s)', got));
END $$;

-- ---------------------------------------------------------------------------
-- 2. Off, nothing changes; on, untagged claims are gone and nothing else is.
-- ---------------------------------------------------------------------------
DO $$
DECLARE got text;
BEGIN
  got := pg_temp.feed(ARRAY['CLAIM','NEWS'], NULL, 100, false);
  PERFORM assert(got = 'c2,n1,c1', format('rule off: every claim, newest first (got %s)', got));
  got := pg_temp.feed(ARRAY['CLAIM','NEWS'], NULL, 100, true);
  PERFORM assert(got = 'n1,c1', format('rule on: the untagged claim is gone, news untouched (got %s)', got));
  got := pg_temp.feed(NULL, NULL, 100, true);
  PERFORM assert(got = 'n1,c1', format('rule on without types gives the same (got %s)', got));
END $$;

-- ---------------------------------------------------------------------------
-- 3. THE REASON FOR THE MIGRATION. With the rule applied after a capped walk, a cap of 1 on
--    claims takes c2 and then filters it out, returning nothing. Inside the walk it returns c1.
-- ---------------------------------------------------------------------------
DO $$
DECLARE got text;
BEGIN
  got := pg_temp.feed(ARRAY['CLAIM'], NULL, 1, true);
  PERFORM assert(got = 'c1', format('a cap of 1 still reaches the tagged claim (got %s)', got));
END $$;

-- ---------------------------------------------------------------------------
-- 4. The tag counts only from a space being read, as in Explore.
-- ---------------------------------------------------------------------------
DO $$
DECLARE got text;
BEGIN
  got := pg_temp.feed(ARRAY['CLAIM','NEWS'], ARRAY['S'], 100, true);
  PERFORM assert(got = 'n1,c1', format('reading S, where the tag was written: c1 is in (got %s)', got));
  got := pg_temp.feed(ARRAY['CLAIM','NEWS'], ARRAY['S2'], 100, true);
  PERFORM assert(got = 'n1', format('reading only S2: the tag does not count (got %s)', got));
  got := pg_temp.feed(NULL, ARRAY['S2'], 100, true);
  PERFORM assert(got = 'n1', format('untyped, reading only S2: the same (got %s)', got));
END $$;

-- ---------------------------------------------------------------------------
-- 5. Tagging and untagging reach the table through the reconcile.
-- ---------------------------------------------------------------------------
DO $$
DECLARE got text;
BEGIN
  PERFORM pg_temp.rel(pg_temp.e('c2'), '25709034-1ba5-406f-94e4-d4af90042fba', pg_temp.c('DEBATE_TAG'), pg_temp.c('S'));
  DELETE FROM relations WHERE from_entity_id = pg_temp.e('c1') AND type_id = '25709034-1ba5-406f-94e4-d4af90042fba';
  PERFORM public.reconcile_entity_topic_ranking();
  got := pg_temp.feed(ARRAY['CLAIM','NEWS'], NULL, 100, true);
  PERFORM assert(got = 'c2,n1', format('c2 tagged and c1 untagged are both picked up (got %s)', got));
END $$;
