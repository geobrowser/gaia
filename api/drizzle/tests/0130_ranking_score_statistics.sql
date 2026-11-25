-- Assertions for 0130: entity_ranking_scores.ranking_score keeps statistics target 10000, and both 56M-row tables
-- auto-analyze at 1%. Catches a later migration that re-creates either table, or resets the settings, which drops
-- them without any error.
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

SELECT assert(a.attstattarget = 10000, '1: entity_ranking_scores.ranking_score has statistics target 10000')
FROM pg_attribute a
WHERE a.attrelid = 'public.entity_ranking_scores'::regclass AND a.attname = 'ranking_score';

SELECT assert(COALESCE('autovacuum_analyze_scale_factor=0.01' = ANY (c.reloptions), false),
              '2: entity_ranking_scores auto-analyzes at 1%')
FROM pg_class c WHERE c.oid = 'public.entity_ranking_scores'::regclass;

SELECT assert(COALESCE('autovacuum_analyze_scale_factor=0.01' = ANY (c.reloptions), false),
              '3: entities auto-analyzes at 1%')
FROM pg_class c WHERE c.oid = 'public.entities'::regclass;
