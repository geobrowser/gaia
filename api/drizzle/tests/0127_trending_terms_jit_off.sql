-- Assertions for 0127: trending.entity_trending_terms_as carries jit = off. This catches a later migration that
-- re-creates the function without `SET jit = off`, which drops the setting without any error.
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

SELECT assert(COALESCE('jit=off' = ANY (p.proconfig), false),
              '1: entity_trending_terms_as runs with jit = off')
FROM pg_proc p
WHERE p.oid = 'trending.entity_trending_terms_as(uuid[], boolean)'::regprocedure;

SELECT assert(COALESCE(NOT ('jit=off' = ANY (p.proconfig)), true),
              '2: entity_trending_terms (the wrapper) has no JIT setting of its own')
FROM pg_proc p
WHERE p.oid = 'trending.entity_trending_terms(uuid[])'::regprocedure;
