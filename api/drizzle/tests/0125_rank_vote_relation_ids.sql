-- Assertions for 0125 (GEO-3591): items are scoped by (ranking_id, rank_space_id), and 0124's
-- trending.ranking_member_ballots joins them on the ballot's own (Rank id, space).
--
--   1. The schema: rank_space_id is NOT NULL and part of the primary key, so one Rank id registered from two
--      spaces can hold the same item twice, once per space.
--   2. A Rank id registered from two member spaces is two ballots: each member's ballot holds only the items
--      written under its own space.
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

-- 1. Schema.
SELECT assert((SELECT is_nullable FROM information_schema.columns
               WHERE table_schema = 'ranks' AND table_name = 'ranking_items'
                 AND column_name = 'rank_space_id') = 'NO',
              '1: ranking_items.rank_space_id is NOT NULL');
SELECT assert((SELECT array_agg(a.attname::text ORDER BY array_position(c.conkey, a.attnum))
               FROM pg_constraint c
               JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = ANY (c.conkey)
               WHERE c.conrelid = 'ranks.ranking_items'::regclass AND c.contype = 'p')
              = ARRAY['ranking_id', 'rank_space_id', 'entity_id', 'space_id'],
              '1: the primary key is (ranking_id, rank_space_id, entity_id, space_id)');

TRUNCATE trending.ranking_place_sources, ranks.rankings, ranks.ranking_items, ranks.members, ranks.editors
         CASCADE;

-- 2. Rank R registered from M1's and M2's spaces, both members of DEB, on the source block.
INSERT INTO trending.ranking_place_sources (name, space_id, block_id, weight, window_days, slots, day_factor)
VALUES ('e2e_0125', '00000000-0000-4000-8125-000000000001', '00000000-0000-4000-8125-000000000002', 1, NULL, 10, 1);
INSERT INTO ranks.members (member_space_id, space_id) VALUES
  ('00000000-0000-4000-8125-000000000011', '00000000-0000-4000-8125-000000000001'),
  ('00000000-0000-4000-8125-000000000012', '00000000-0000-4000-8125-000000000001');
INSERT INTO ranks.rankings (id, block_id, space_id, updated_at_block, update_index) VALUES
  ('00000000-0000-4000-8125-000000000041', '00000000-0000-4000-8125-000000000002',
   '00000000-0000-4000-8125-000000000011', 10, 0),
  ('00000000-0000-4000-8125-000000000041', '00000000-0000-4000-8125-000000000002',
   '00000000-0000-4000-8125-000000000012', 10, 0);
-- M1's ballot holds A; M2's holds C, and A too (the same item under each space is allowed).
INSERT INTO ranks.ranking_items (ranking_id, rank_space_id, entity_id, space_id) VALUES
  ('00000000-0000-4000-8125-000000000041', '00000000-0000-4000-8125-000000000011',
   '00000000-0000-4000-8125-0000000000a1', '00000000-0000-4000-8125-000000000001'),
  ('00000000-0000-4000-8125-000000000041', '00000000-0000-4000-8125-000000000012',
   '00000000-0000-4000-8125-0000000000c1', '00000000-0000-4000-8125-000000000001'),
  ('00000000-0000-4000-8125-000000000041', '00000000-0000-4000-8125-000000000012',
   '00000000-0000-4000-8125-0000000000a1', '00000000-0000-4000-8125-000000000001');

SELECT assert((SELECT array_agg(entity_id::text ORDER BY entity_id) FROM trending.ranking_member_ballots()
               WHERE member_space_id = '00000000-0000-4000-8125-000000000011')
              = ARRAY['00000000-0000-4000-8125-0000000000a1'],
              '2: M1''s ballot holds only the item written under M1''s space');
SELECT assert((SELECT array_agg(entity_id::text ORDER BY entity_id) FROM trending.ranking_member_ballots()
               WHERE member_space_id = '00000000-0000-4000-8125-000000000012')
              = ARRAY['00000000-0000-4000-8125-0000000000a1', '00000000-0000-4000-8125-0000000000c1'],
              '2: M2''s ballot holds its own two items');
