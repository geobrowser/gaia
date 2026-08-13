\set ON_ERROR_STOP on
CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT cond THEN RAISE EXCEPTION 'FAIL: %', label; ELSE RAISE NOTICE 'pass: %', label; END IF; END; $$;

TRUNCATE entities, values, relations, votes_count, entity_ranking_scores,
         entity_type_weights, entity_type_exclusions, entity_feed_blocklist;

-- Spaces A and B; types T1, T2, and TX (globally excluded).
-- Entity naming is what carries space membership — see 0077's header for why space
-- scoping is folded into the name probe rather than added as a separate predicate.
INSERT INTO entities (id, created_at) VALUES
  ('e1111111-1111-1111-1111-111111111111','1786000000'),  -- name in A,   type T1
  ('e2222222-2222-2222-2222-222222222222','1786000000'),  -- name in B,   type T1
  ('e3333333-3333-3333-3333-333333333333','1786000000'),  -- name in A,   type T2
  ('e4444444-4444-4444-4444-444444444444','1786000000'),  -- name in A+B, type T1
  ('e5555555-5555-5555-5555-555555555555','1786000000'),  -- name in A,   type TX (excluded)
  ('e6666666-6666-6666-6666-666666666666','1786000000'),  -- name in A,   UNTYPED
  ('e7777777-7777-7777-7777-777777777777','1786000000');  -- name in A,   type T1, BLOCKLISTED

INSERT INTO values (id, property_id, entity_id, space_id, text) VALUES
  ('n1','a126ca53-0c8e-48d5-b888-82c734c38935','e1111111-1111-1111-1111-111111111111','aaaaaaaa-0000-0000-0000-00000000000a','In A only'),
  ('n2','a126ca53-0c8e-48d5-b888-82c734c38935','e2222222-2222-2222-2222-222222222222','bbbbbbbb-0000-0000-0000-00000000000b','In B only'),
  ('n3','a126ca53-0c8e-48d5-b888-82c734c38935','e3333333-3333-3333-3333-333333333333','aaaaaaaa-0000-0000-0000-00000000000a','In A, type T2'),
  ('n4a','a126ca53-0c8e-48d5-b888-82c734c38935','e4444444-4444-4444-4444-444444444444','aaaaaaaa-0000-0000-0000-00000000000a','In both, A name'),
  ('n4b','a126ca53-0c8e-48d5-b888-82c734c38935','e4444444-4444-4444-4444-444444444444','bbbbbbbb-0000-0000-0000-00000000000b','In both, B name'),
  ('n5','a126ca53-0c8e-48d5-b888-82c734c38935','e5555555-5555-5555-5555-555555555555','aaaaaaaa-0000-0000-0000-00000000000a','Excluded type'),
  ('n6','a126ca53-0c8e-48d5-b888-82c734c38935','e6666666-6666-6666-6666-666666666666','aaaaaaaa-0000-0000-0000-00000000000a','Untyped'),
  ('n7','a126ca53-0c8e-48d5-b888-82c734c38935','e7777777-7777-7777-7777-777777777777','aaaaaaaa-0000-0000-0000-00000000000a','Blocklisted');

-- TYPES relations. d1d1d1d1… = T1, d2d2d2d2… = T2, dededede… = TX (globally excluded).
INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system) VALUES
  ('0a000001-0000-0000-0000-000000000001','ee000000-0000-0000-0000-000000000001','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e1111111-1111-1111-1111-111111111111','d1d1d1d1-0000-0000-0000-000000000001','aaaaaaaa-0000-0000-0000-00000000000a',false),
  ('0a000002-0000-0000-0000-000000000002','ee000000-0000-0000-0000-000000000002','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e2222222-2222-2222-2222-222222222222','d1d1d1d1-0000-0000-0000-000000000001','bbbbbbbb-0000-0000-0000-00000000000b',false),
  ('0a000003-0000-0000-0000-000000000003','ee000000-0000-0000-0000-000000000003','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e3333333-3333-3333-3333-333333333333','d2d2d2d2-0000-0000-0000-000000000002','aaaaaaaa-0000-0000-0000-00000000000a',false),
  ('0a000004-0000-0000-0000-000000000004','ee000000-0000-0000-0000-000000000004','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e4444444-4444-4444-4444-444444444444','d1d1d1d1-0000-0000-0000-000000000001','aaaaaaaa-0000-0000-0000-00000000000a',false),
  ('0a000005-0000-0000-0000-000000000005','ee000000-0000-0000-0000-000000000005','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e5555555-5555-5555-5555-555555555555','dededede-0000-0000-0000-00000000000e','aaaaaaaa-0000-0000-0000-00000000000a',false),
  ('0a000007-0000-0000-0000-000000000007','ee000000-0000-0000-0000-000000000007','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e7777777-7777-7777-7777-777777777777','d1d1d1d1-0000-0000-0000-000000000001','aaaaaaaa-0000-0000-0000-00000000000a',false);

INSERT INTO entity_type_exclusions (type_id, note) VALUES ('dededede-0000-0000-0000-00000000000e','TX is globally excluded');
INSERT INTO entity_feed_blocklist (entity_id, reason) VALUES ('e7777777-7777-7777-7777-777777777777','moderated');

SELECT refresh_entity_ranking_scores(ARRAY[
  'e1111111-1111-1111-1111-111111111111','e2222222-2222-2222-2222-222222222222',
  'e3333333-3333-3333-3333-333333333333','e4444444-4444-4444-4444-444444444444',
  'e5555555-5555-5555-5555-555555555555','e6666666-6666-6666-6666-666666666666',
  'e7777777-7777-7777-7777-777777777777']::uuid[]);

-- ---- backward compatibility: the added params are optional ----
SELECT assert((SELECT count(*) FROM entities_ranked_for_feed()) = 5,
  'unscoped: all servable entities remain (TX excluded, blocklisted dropped)');
SELECT assert((SELECT count(*) FROM entities_ranked_for_feed(NULL, '1785000000')) = 5,
  'positional pre-0077 call shape still resolves (recency arg, no space/type)');

-- ---- space scoping ----
SELECT assert((SELECT count(*) FROM entities_ranked_for_feed(NULL,NULL,NULL,ARRAY['aaaaaaaa-0000-0000-0000-00000000000a']::uuid[])) = 4,
  'space A serves the 4 entities named in A');
SELECT assert(NOT EXISTS (SELECT 1 FROM entities_ranked_for_feed(NULL,NULL,NULL,ARRAY['aaaaaaaa-0000-0000-0000-00000000000a']::uuid[])
                          WHERE id='e2222222-2222-2222-2222-222222222222'),
  'an entity named ONLY in B is not served to A — it would render as a raw uuid there');
SELECT assert((SELECT count(*) FROM entities_ranked_for_feed(NULL,NULL,NULL,ARRAY['bbbbbbbb-0000-0000-0000-00000000000b']::uuid[])) = 2,
  'space B serves only the B-named entities');
SELECT assert(EXISTS (SELECT 1 FROM entities_ranked_for_feed(NULL,NULL,NULL,ARRAY['bbbbbbbb-0000-0000-0000-00000000000b']::uuid[])
                      WHERE id='e4444444-4444-4444-4444-444444444444'),
  'an entity named in BOTH spaces is served to either');
SELECT assert((SELECT count(*) FROM entities_ranked_for_feed(NULL,NULL,NULL,
    ARRAY['aaaaaaaa-0000-0000-0000-00000000000a','bbbbbbbb-0000-0000-0000-00000000000b']::uuid[])) = 5,
  'both spaces together match the unscoped result (no double-counting from the two-name entity)');
SELECT assert((SELECT count(*) FROM entities_ranked_for_feed(NULL,NULL,NULL,ARRAY['cccccccc-0000-0000-0000-00000000000c']::uuid[])) = 0,
  'a space with no named entities serves nothing');

-- ---- type scoping ----
SELECT assert((SELECT count(*) FROM entities_ranked_for_feed(NULL,NULL,NULL,NULL,ARRAY['d1d1d1d1-0000-0000-0000-000000000001']::uuid[])) = 3,
  'type T1 serves its 3 servable entities');
SELECT assert(NOT EXISTS (SELECT 1 FROM entities_ranked_for_feed(NULL,NULL,NULL,NULL,ARRAY['d1d1d1d1-0000-0000-0000-000000000001']::uuid[])
                          WHERE id='e6666666-6666-6666-6666-666666666666'),
  'type_ids is a positive filter: an UNTYPED entity is excluded when types are named');
SELECT assert((SELECT count(*) FROM entities_ranked_for_feed(NULL,NULL,NULL,NULL,
    ARRAY['d1d1d1d1-0000-0000-0000-000000000001','d2d2d2d2-0000-0000-0000-000000000002']::uuid[])) = 4,
  'type_ids has OR semantics across the set');

-- ---- the invariant that must not be bypassable ----
SELECT assert((SELECT count(*) FROM entities_ranked_for_feed(NULL,NULL,NULL,NULL,ARRAY['dededede-0000-0000-0000-00000000000e']::uuid[])) = 0,
  'asking for a GLOBALLY EXCLUDED type serves nothing — exclusion is a feed invariant, not a default a caller can opt out of');
SELECT assert(NOT EXISTS (SELECT 1 FROM entities_ranked_for_feed(NULL,NULL,NULL,
    ARRAY['aaaaaaaa-0000-0000-0000-00000000000a']::uuid[], ARRAY['d1d1d1d1-0000-0000-0000-000000000001']::uuid[])
    WHERE id='e7777777-7777-7777-7777-777777777777'),
  'the blocklist still wins when both space and type scoping are supplied');

-- ---- combined ----
SELECT assert((SELECT count(*) FROM entities_ranked_for_feed(NULL,NULL,NULL,
    ARRAY['aaaaaaaa-0000-0000-0000-00000000000a']::uuid[], ARRAY['d1d1d1d1-0000-0000-0000-000000000001']::uuid[])) = 2,
  'space and type scoping intersect (AND), not union');
SELECT assert((SELECT count(*) FROM entities_ranked_for_feed(NULL,NULL,NULL,
    ARRAY['bbbbbbbb-0000-0000-0000-00000000000b']::uuid[], ARRAY['d2d2d2d2-0000-0000-0000-000000000002']::uuid[])) = 0,
  'a space/type pair with no members serves nothing');

-- ---- ordering survives scoping ----
SELECT assert((SELECT count(*) FROM (
    SELECT ranking_score FROM entities_ranked_for_feed(NULL,NULL,NULL,ARRAY['aaaaaaaa-0000-0000-0000-00000000000a']::uuid[]) f
    JOIN entity_ranking_scores rs ON rs.entity_id=f.id
  ) x) = 4,
  'every scoped row still has its score row (the join is not dropped by scoping)');
SELECT assert((
    SELECT bool_and(prev >= cur) FROM (
      SELECT rs.ranking_score AS cur,
             lag(rs.ranking_score) OVER () AS prev
      FROM entities_ranked_for_feed(NULL,NULL,NULL,ARRAY['aaaaaaaa-0000-0000-0000-00000000000a']::uuid[]) f
      JOIN entity_ranking_scores rs ON rs.entity_id=f.id
    ) s WHERE prev IS NOT NULL
  ) IS NOT FALSE,
  'scoped results are still returned in descending ranking_score order');

\echo '=== 0077 ASSERTIONS PASSED ==='
