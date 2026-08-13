\set ON_ERROR_STOP on
CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT cond THEN RAISE EXCEPTION 'FAIL: %', label; ELSE RAISE NOTICE 'pass: %', label; END IF; END; $$;

TRUNCATE entities, values, relations, votes_count, entity_ranking_scores,
         entity_type_weights, entity_type_exclusions, entity_feed_blocklist;

INSERT INTO entities (id, created_at) VALUES
  ('11111111-1111-1111-1111-111111111111','1786000000'),  -- named
  ('22222222-2222-2222-2222-222222222222','1786000000'),  -- NO values at all (empty shell)
  ('33333333-3333-3333-3333-333333333333','1786000000'),  -- description but NO name
  ('44444444-4444-4444-4444-444444444444','1786000000'),  -- name is empty string
  ('55555555-5555-5555-5555-555555555555','1786000000');  -- name is whitespace only
INSERT INTO values (id, property_id, entity_id, space_id, text) VALUES
  ('v1','a126ca53-0c8e-48d5-b888-82c734c38935','11111111-1111-1111-1111-111111111111','aaaaaaaa-0000-0000-0000-000000000000','Real Entity'),
  ('v3','9b1f76ff-9711-404c-861e-59dc3fa7d037','33333333-3333-3333-3333-333333333333','aaaaaaaa-0000-0000-0000-000000000000','a description but no name'),
  ('v4','a126ca53-0c8e-48d5-b888-82c734c38935','44444444-4444-4444-4444-444444444444','aaaaaaaa-0000-0000-0000-000000000000',''),
  ('v5','a126ca53-0c8e-48d5-b888-82c734c38935','55555555-5555-5555-5555-555555555555','aaaaaaaa-0000-0000-0000-000000000000','   ');

SELECT assert(refresh_entity_ranking_scores(ARRAY[
  '11111111-1111-1111-1111-111111111111','22222222-2222-2222-2222-222222222222',
  '33333333-3333-3333-3333-333333333333','44444444-4444-4444-4444-444444444444',
  '55555555-5555-5555-5555-555555555555']::uuid[]) = 5,
  'all 5 are SCORED — the name rule is a serving rule, not a scoring rule');

SELECT assert((SELECT count(*) FROM entities_ranked_for_feed()) = 1,
  'only the named entity is servable');
SELECT assert((SELECT id FROM entities_ranked_for_feed()) = '11111111-1111-1111-1111-111111111111',
  'and it is the right one');
SELECT assert(NOT EXISTS (SELECT 1 FROM entities_ranked_for_feed() WHERE id='22222222-2222-2222-2222-222222222222'),
  'an empty shell with no values is never served — this was 97.8% of the corpus');
SELECT assert(NOT EXISTS (SELECT 1 FROM entities_ranked_for_feed() WHERE id='33333333-3333-3333-3333-333333333333'),
  'a description without a name is NOT enough — it still renders as a raw id');
SELECT assert(NOT EXISTS (SELECT 1 FROM entities_ranked_for_feed() WHERE id='44444444-4444-4444-4444-444444444444'),
  'an empty-string name does not count as a name');
SELECT assert(NOT EXISTS (SELECT 1 FROM entities_ranked_for_feed() WHERE id='55555555-5555-5555-5555-555555555555'),
  'a whitespace-only name does not count as a name');
\echo '=== 0075 ASSERTIONS PASSED ==='
