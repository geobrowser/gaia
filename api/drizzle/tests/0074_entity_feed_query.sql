\set ON_ERROR_STOP on
CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT cond THEN RAISE EXCEPTION 'FAIL: %', label; ELSE RAISE NOTICE 'pass: %', label; END IF; END; $$;

TRUNCATE entities, values, relations, votes_count, entity_ranking_scores,
         entity_type_weights, entity_type_exclusions, entity_feed_blocklist;

INSERT INTO entities (id, created_at) VALUES
  ('11111111-1111-1111-1111-111111111111','1786000000'),  -- high score, servable
  ('22222222-2222-2222-2222-222222222222','1786000000'),  -- BLOCKLISTED
  ('33333333-3333-3333-3333-333333333333','1786000000'),  -- EXCLUDED TYPE
  ('44444444-4444-4444-4444-444444444444','1768000000'),  -- old (outside window)
  ('55555555-5555-5555-5555-555555555555','1786000000');  -- no score row at all
-- Every fixture needs a name to be servable at all (0075). Without these the suite
-- asserts nothing: all five entities fail candidate generation on the name rule and
-- the blocklist/exclusion assertions below pass vacuously.
INSERT INTO values (id, property_id, entity_id, space_id, text) VALUES
  ('m1','a126ca53-0c8e-48d5-b888-82c734c38935','11111111-1111-1111-1111-111111111111','aaaaaaaa-0000-0000-0000-000000000000','Servable'),
  ('m2','a126ca53-0c8e-48d5-b888-82c734c38935','22222222-2222-2222-2222-222222222222','aaaaaaaa-0000-0000-0000-000000000000','Blocklisted'),
  ('m3','a126ca53-0c8e-48d5-b888-82c734c38935','33333333-3333-3333-3333-333333333333','aaaaaaaa-0000-0000-0000-000000000000','Excluded type'),
  ('m4','a126ca53-0c8e-48d5-b888-82c734c38935','44444444-4444-4444-4444-444444444444','aaaaaaaa-0000-0000-0000-000000000000','Old'),
  ('m5','a126ca53-0c8e-48d5-b888-82c734c38935','55555555-5555-5555-5555-555555555555','aaaaaaaa-0000-0000-0000-000000000000','No score row');
INSERT INTO votes_count (object_id, object_type, space_id, vote_kind, positive, negative) VALUES
  ('11111111-1111-1111-1111-111111111111',0,'aaaaaaaa-0000-0000-0000-000000000000',0,50,2),
  ('22222222-2222-2222-2222-222222222222',0,'aaaaaaaa-0000-0000-0000-000000000000',0,99,0),
  ('33333333-3333-3333-3333-333333333333',0,'aaaaaaaa-0000-0000-0000-000000000000',0,99,0);
-- entity 3 declares an excluded type
INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system) VALUES
  ('0b111111-1111-1111-1111-111111111111'::uuid,'e0000000-0000-0000-0000-000000000001'::uuid,
   '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','33333333-3333-3333-3333-333333333333',
   'dddddddd-0000-0000-0000-000000000001','aaaaaaaa-0000-0000-0000-000000000000',false);
INSERT INTO entity_type_exclusions (type_id, note) VALUES ('dddddddd-0000-0000-0000-000000000001','Text block');
INSERT INTO entity_feed_blocklist (entity_id, reason) VALUES ('22222222-2222-2222-2222-222222222222','moderated');

SELECT refresh_entity_ranking_scores(ARRAY[
  '11111111-1111-1111-1111-111111111111','22222222-2222-2222-2222-222222222222',
  '33333333-3333-3333-3333-333333333333','44444444-4444-4444-4444-444444444444']::uuid[]);

-- ---- candidate generation ----
SELECT assert(NOT EXISTS (SELECT 1 FROM entities_ranked_for_feed() WHERE id='55555555-5555-5555-5555-555555555555'),
  'an entity with no score row is absent from the feed (inner join, not left)');
SELECT assert(NOT EXISTS (SELECT 1 FROM entities_ranked_for_feed() WHERE id='22222222-2222-2222-2222-222222222222'),
  'a BLOCKLISTED entity is never served, despite scoring 99-0 — the highest quality in the fixture');
SELECT assert(NOT EXISTS (SELECT 1 FROM entities_ranked_for_feed() WHERE id='33333333-3333-3333-3333-333333333333'),
  'an EXCLUDED-TYPE entity is never served, despite scoring 99-0');
SELECT assert((SELECT count(*) FROM entities_ranked_for_feed()) = 2,
  'after both exclusions only the 2 servable entities remain');
SELECT assert((SELECT id FROM entities_ranked_for_feed() LIMIT 1) = '11111111-1111-1111-1111-111111111111',
  'top of feed is the highest-ranked servable entity');

-- ---- filters ----
SELECT assert((SELECT count(*) FROM entities_ranked_for_feed(NULL, '1785000000')) = 1,
  'created_after excludes the old entity (recency window)');
SELECT assert((SELECT count(*) FROM entities_ranked_for_feed(NULL, NULL, '1770000000')) = 1,
  'created_before keeps only the old entity');
SELECT assert((SELECT count(*) FROM entities_ranked_for_feed(999999)) = 0,
  'an impossible min_ranking_score returns nothing rather than erroring');
SELECT assert((SELECT count(*) FROM entities_ranked_for_feed(
                 (SELECT ranking_score FROM entity_ranking_scores
                   WHERE entity_id='11111111-1111-1111-1111-111111111111'))) = 1,
  'min_ranking_score is inclusive at the boundary');

-- ---- ordering is total (cursor pagination safety) ----
SELECT assert((SELECT count(DISTINCT (rs.ranking_score, rs.entity_id)) FROM entity_ranking_scores rs)
            = (SELECT count(*) FROM entity_ranking_scores),
  '(ranking_score, entity_id) is unique — a total order, so cursor paging cannot skip or duplicate');

-- ---- computed columns ----
SELECT assert((SELECT entities_ranking_score(e) FROM entities e WHERE e.id='11111111-1111-1111-1111-111111111111') IS NOT NULL,
  'entities_ranking_score computed column resolves');
SELECT assert((SELECT entities_upvotes(e) FROM entities e WHERE e.id='11111111-1111-1111-1111-111111111111') = 50,
  'entities_upvotes exposes the curation positive tally');
SELECT assert((SELECT entities_downvotes(e) FROM entities e WHERE e.id='11111111-1111-1111-1111-111111111111') = 2,
  'entities_downvotes exposes the curation negative tally');
SELECT assert((SELECT entities_ranking_score(e) FROM entities e WHERE e.id='55555555-5555-5555-5555-555555555555') IS NULL,
  'an unscored entity yields NULL rather than 0 — so it sorts last under nulls:last, not first');
\echo '=== 0074 ASSERTIONS PASSED ==='
