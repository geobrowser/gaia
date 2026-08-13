\set ON_ERROR_STOP on
CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT cond THEN RAISE EXCEPTION 'FAIL: %', label; ELSE RAISE NOTICE 'pass: %', label; END IF; END; $$;

TRUNCATE entities, values, relations, votes_count, entity_ranking_scores,
         entity_type_weights, entity_type_exclusions, entity_feed_blocklist;

INSERT INTO entities (id, created_at) VALUES
  ('11111111-1111-1111-1111-111111111111','1786000000'),  -- normal, named
  ('22222222-2222-2222-2222-222222222222','1786000000'),  -- SYSTEM entity
  ('33333333-3333-3333-3333-333333333333','1786000000');  -- user Type named "Space", NOT system
INSERT INTO values (id, property_id, entity_id, space_id, text) VALUES
  ('v1','a126ca53-0c8e-48d5-b888-82c734c38935','11111111-1111-1111-1111-111111111111','aaaaaaaa-0000-0000-0000-000000000000','Real Article'),
  ('v2','a126ca53-0c8e-48d5-b888-82c734c38935','22222222-2222-2222-2222-222222222222','aaaaaaaa-0000-0000-0000-000000000000','Some Space System Entity'),
  ('v3','a126ca53-0c8e-48d5-b888-82c734c38935','33333333-3333-3333-3333-333333333333','aaaaaaaa-0000-0000-0000-000000000000','A user-typed Space');
-- entity 2 carries the unforgeable System Type relation
INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system) VALUES
  ('0b111111-1111-1111-1111-111111111111','e0000000-0000-0000-0000-000000000001',
   '88b3d6ad-288c-529c-a212-0e1c24819185','22222222-2222-2222-2222-222222222222',
   'cccccccc-0000-0000-0000-000000000001','aaaaaaaa-0000-0000-0000-000000000000',true),
-- entity 3 declares a normal user Type pointing at an entity named "Space" — this
-- must NOT be treated as a system entity. This is the forgery case the marker prevents.
  ('0b222222-2222-2222-2222-222222222222','e0000000-0000-0000-0000-000000000002',
   '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','33333333-3333-3333-3333-333333333333',
   'dddddddd-0000-0000-0000-000000000001','aaaaaaaa-0000-0000-0000-000000000000',false);

SELECT assert(refresh_entity_ranking_scores(ARRAY[
  '11111111-1111-1111-1111-111111111111','22222222-2222-2222-2222-222222222222',
  '33333333-3333-3333-3333-333333333333']::uuid[]) = 3,
  'all 3 are SCORED — excluding system entities is a serving rule, not a scoring rule');

SELECT assert(NOT EXISTS (SELECT 1 FROM entities_ranked_for_feed() WHERE id='22222222-2222-2222-2222-222222222222'),
  'a System Type entity is never served, even though it has a perfectly good name');
SELECT assert(EXISTS (SELECT 1 FROM entities_ranked_for_feed() WHERE id='33333333-3333-3333-3333-333333333333'),
  'a user-typed entity is NOT treated as a system entity — the marker cannot be forged via a Type relation');
SELECT assert(EXISTS (SELECT 1 FROM entities_ranked_for_feed() WHERE id='11111111-1111-1111-1111-111111111111'),
  'normal content is unaffected');
SELECT assert((SELECT count(*) FROM entities_ranked_for_feed()) = 2, 'exactly 2 servable');

-- and the name-keyed exclusion still works independently (Payout has no system marker)
INSERT INTO entity_type_exclusions (type_id, note, updated_at)
  VALUES ('dddddddd-0000-0000-0000-000000000001','user-typed Space excluded by name', now());
SELECT assert(NOT EXISTS (SELECT 1 FROM entities_ranked_for_feed() WHERE id='33333333-3333-3333-3333-333333333333'),
  'name-keyed exclusion still applies on top — the two mechanisms are independent');
\echo '=== 0076 ASSERTIONS PASSED ==='
