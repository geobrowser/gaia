-- Assertions for 0110: public.search takes candidate ids from the caller.
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
--   1. One function: the old three-argument overload is gone, so a three-argument call is
--      unambiguous.
--   2. ids given: the rows come back in the order given, whatever the trigram score would say;
--      query, space_id and similarity_threshold are ignored; unknown ids drop out; a duplicated id
--      appears once, at its first position; an empty array returns nothing.
--   3. ids omitted or NULL: the legacy trigram body, best similarity first.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

TRUNCATE entities, "values", spaces CASCADE;

INSERT INTO spaces (id, type, address) VALUES ('00000000-0000-4000-8000-0000000000a1', 'Personal', '0xa1');

INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block) VALUES
  ('00000000-0000-4000-8000-000000000001', '0', '0', '0', '0'),
  ('00000000-0000-4000-8000-000000000002', '0', '0', '0', '0'),
  ('00000000-0000-4000-8000-000000000003', '0', '0', '0', '0');

-- Names: 1 is the best trigram match for 'zebra crossing', 3 the worst that still clears 0.3.
INSERT INTO "values" (id, property_id, entity_id, space_id, "text") VALUES
  ('v1', 'a126ca53-0c8e-48d5-b888-82c734c38935', '00000000-0000-4000-8000-000000000001', '00000000-0000-4000-8000-0000000000a1', 'zebra crossing'),
  ('v2', 'a126ca53-0c8e-48d5-b888-82c734c38935', '00000000-0000-4000-8000-000000000002', '00000000-0000-4000-8000-0000000000a1', 'zebra crossings'),
  ('v3', 'a126ca53-0c8e-48d5-b888-82c734c38935', '00000000-0000-4000-8000-000000000003', '00000000-0000-4000-8000-0000000000a1', 'zebra crossing guide for pedestrians');

SELECT assert((SELECT count(*) FROM pg_proc WHERE proname = 'search' AND pronamespace = 'public'::regnamespace) = 1,
              '1: exactly one public.search overload');

-- 3. Legacy body, reached with three arguments, with NULL, and with the default.
SELECT assert((SELECT array_agg(id ORDER BY ord) FROM (SELECT id, row_number() OVER () ord FROM public.search('zebra crossing', NULL, 0.3)) s)
              = ARRAY['00000000-0000-4000-8000-000000000001', '00000000-0000-4000-8000-000000000002',
                      '00000000-0000-4000-8000-000000000003']::uuid[],
              '3: three-argument call runs the trigram body, best match first');
SELECT assert((SELECT count(*) FROM public.search('zebra crossing')) = 3, '3: defaults still resolve');
SELECT assert((SELECT count(*) FROM public.search('zebra crossing', NULL, 0.3, NULL)) = 3, '3: ids NULL is the legacy body');

-- 2. Supplied ids.
SELECT assert((SELECT array_agg(id) FROM (SELECT id FROM public.search('x', NULL, NULL,
                 ARRAY['00000000-0000-4000-8000-000000000003', '00000000-0000-4000-8000-000000000001',
                       '00000000-0000-4000-8000-000000000002']::uuid[])) s)
              = ARRAY['00000000-0000-4000-8000-000000000003', '00000000-0000-4000-8000-000000000001',
                      '00000000-0000-4000-8000-000000000002']::uuid[],
              '2: rows come back in the order given, ignoring query and threshold');
SELECT assert((SELECT count(*) FROM public.search('no such text', NULL, 0.99,
                 ARRAY['00000000-0000-4000-8000-000000000001']::uuid[])) = 1,
              '2: ids bypass the trigram match entirely');
SELECT assert((SELECT array_agg(id) FROM (SELECT id FROM public.search('x', NULL, NULL,
                 ARRAY['00000000-0000-4000-8000-0000000000ff', '00000000-0000-4000-8000-000000000002',
                       '00000000-0000-4000-8000-000000000002', '00000000-0000-4000-8000-000000000001']::uuid[])) s)
              = ARRAY['00000000-0000-4000-8000-000000000002', '00000000-0000-4000-8000-000000000001']::uuid[],
              '2: unknown ids drop out; a duplicate appears once, at its first position');
SELECT assert((SELECT count(*) FROM public.search('zebra crossing', NULL, 0.3, ARRAY[]::uuid[])) = 0,
              '2: an empty array returns nothing, not the legacy result');
