-- Assertions for 0116: geodb.conform is the same hash as conformingId in TypeScript, and the reverse
-- lookup api/src/geodb/idMap.ts runs finds legacy ids through the index.
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
--   1. Known pairs. The same constants are asserted against conformingId in
--      api/src/geodb/__tests__/idMap.test.ts, so the two implementations cannot drift apart.
--   2. v4 ids pass through; the kind is part of the hash.
--   3. The lookup query maps a legacy id back, finds a native id as itself, and finds nothing for
--      an id neither holds.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

TRUNCATE entities CASCADE;

-- 1. Known pairs (a v5 id from testnet, a v7, an all-ones id, and the statement kind).
SELECT assert(geodb.conform('entity', '990a4147-a928-55e8-8e63-0e542faf1c72') = 'a523681b-ad07-4d0d-9b43-e0f6d117406a', 'v5 entity id');
SELECT assert(geodb.conform('entity', '01a1a8c0-0000-7000-8000-000000000001') = 'facd0857-ad85-49a6-968b-410df35ba70a', 'v7 entity id');
SELECT assert(geodb.conform('entity', 'ffffffff-ffff-ffff-ffff-ffffffffffff') = 'd4f3d63b-ac72-4230-8c2b-906b54a98da2', 'non-RFC entity id');
SELECT assert(geodb.conform('statement', '990a4147-a928-55e8-8e63-0e542faf1c72') = '9b8a3f0c-d576-4ff8-8738-2d0766bebd6f', 'statement kind');

-- 2. Pass-through and the v4 test.
SELECT assert(geodb.conform('entity', '6b9f649e-38b2-4b9a-8e3c-1b6a0b3c3c3a') = '6b9f649e-38b2-4b9a-8e3c-1b6a0b3c3c3a', 'v4 kept');
SELECT assert(geodb.is_v4('6b9f649e-38b2-4b9a-8e3c-1b6a0b3c3c3a'), 'v4 is v4');
SELECT assert(NOT geodb.is_v4('6b9f649e-38b2-4b9a-0e3c-1b6a0b3c3c3a'), 'wrong variant is not v4');
SELECT assert(NOT geodb.is_v4('990a4147-a928-55e8-8e63-0e542faf1c72'), 'v5 is not v4');
SELECT assert(geodb.is_v4(geodb.conform('entity', '990a4147-a928-55e8-8e63-0e542faf1c72')), 'a conformed id is v4');

-- 3. The lookup, as idMap.ts runs it.
INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block) VALUES
  ('990a4147-a928-55e8-8e63-0e542faf1c72', '0', '0', '0', '0'),
  ('6b9f649e-38b2-4b9a-8e3c-1b6a0b3c3c3a', '0', '0', '0', '0');

CREATE TEMP TABLE found AS
SELECT replace(geodb.conform('entity', id)::text, '-', '') AS geo, replace(id::text, '-', '') AS gaia FROM entities
 WHERE geodb.conform('entity', id) = ANY('{a523681bad074d0d9b43e0f6d117406a,6b9f649e38b24b9a8e3c1b6a0b3c3c3a,0d4f1a2b3c4d4e5f8a6b7c8d9e0f1a2b}'::uuid[])
   AND NOT geodb.is_v4(id)
UNION ALL
SELECT replace(id::text, '-', ''), replace(id::text, '-', '') FROM entities
 WHERE id = ANY('{a523681bad074d0d9b43e0f6d117406a,6b9f649e38b24b9a8e3c1b6a0b3c3c3a,0d4f1a2b3c4d4e5f8a6b7c8d9e0f1a2b}'::uuid[])
   AND geodb.is_v4(id);

SELECT assert((SELECT gaia FROM found WHERE geo = 'a523681bad074d0d9b43e0f6d117406a') = '990a4147a92855e88e630e542faf1c72', 'legacy id maps back');
SELECT assert((SELECT gaia FROM found WHERE geo = '6b9f649e38b24b9a8e3c1b6a0b3c3c3a') = '6b9f649e38b24b9a8e3c1b6a0b3c3c3a', 'native id is itself');
SELECT assert((SELECT count(*) FROM found) = 2, 'an unknown id finds nothing');

TRUNCATE entities CASCADE;
DROP TABLE found;
