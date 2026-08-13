\set ON_ERROR_STOP on
CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF NOT cond THEN RAISE EXCEPTION 'FAIL: %', label; ELSE RAISE NOTICE 'pass: %', label; END IF; END; $$;

-- ---- Wilson properties ----
SELECT assert(wilson_lower_bound(0,0) > 0 AND wilson_lower_bound(0,0) < 1,
  'zero-vote entity lands strictly inside (0,1) via the prior, not at the floor');
SELECT assert(wilson_lower_bound(10,0) > wilson_lower_bound(1,0),
  'more upvotes ranks higher');
SELECT assert(wilson_lower_bound(10,0) > wilson_lower_bound(10,5),
  'downvotes reduce quality');
SELECT assert(wilson_lower_bound(0,10) < wilson_lower_bound(0,0),
  'downvote-only ranks BELOW an unvoted entity (the abs() bug in the dead hot score got this wrong)');
SELECT assert(wilson_lower_bound(1,0) < wilson_lower_bound(50,2),
  'PRD acceptance: a low-vote newcomer does NOT outrank a well-established high-quality entity');
SELECT assert(wilson_lower_bound(100,90) < wilson_lower_bound(10,0),
  'net-positive-but-contested ranks below unanimous (net score would tie at +10)');
SELECT assert(wilson_lower_bound(-5,-5) BETWEEN 0 AND 1, 'negative inputs are clamped, not NaN');
SELECT assert(wilson_lower_bound(0,0,1.96,0,0) = 0, 'no observations and no prior yields 0, not a divide-by-zero');

-- ---- intrinsic ----
SELECT assert(entity_intrinsic_score(0,0) = 0, 'bare entity scores 0 intrinsic');
SELECT assert(entity_intrinsic_score(100,100) <= 0.5, 'intrinsic respects its cap');
SELECT assert(entity_intrinsic_score(10,5) > entity_intrinsic_score(2,1), 'richer entity scores higher');
SELECT assert(entity_intrinsic_score(1000,1000) = entity_intrinsic_score(10,5), 'saturates at target, no runaway');

-- ---- fixtures ----
INSERT INTO entities (id, created_at) VALUES
  ('11111111-1111-1111-1111-111111111111','1786000000'),  -- popular, recent
  ('22222222-2222-2222-2222-222222222222','1786000000'),  -- unvoted, recent, rich
  ('33333333-3333-3333-3333-333333333333','1768000000'),  -- popular, OLD
  ('44444444-4444-4444-4444-444444444444','1786000000'),  -- downvoted, recent
  ('55555555-5555-5555-5555-555555555555','1786000000'),  -- typed + boosted
  ('66666666-6666-6666-6666-666666666666','1786000000');  -- ONLY system relations  -- typed + boosted
INSERT INTO votes_count (object_id, object_type, space_id, vote_kind, positive, negative) VALUES
  ('11111111-1111-1111-1111-111111111111',0,'aaaaaaaa-0000-0000-0000-000000000000',0,50,2),
  ('33333333-3333-3333-3333-333333333333',0,'aaaaaaaa-0000-0000-0000-000000000000',0,50,2),
  ('44444444-4444-4444-4444-444444444444',0,'aaaaaaaa-0000-0000-0000-000000000000',0,0,40),
  -- vote_kind=1 (stance) must be ignored by the feed's quality term
  ('22222222-2222-2222-2222-222222222222',0,'aaaaaaaa-0000-0000-0000-000000000000',1,999,0);
INSERT INTO values (id, property_id, entity_id, space_id) VALUES
  ('v1','0a111111-1111-1111-1111-111111111111'::uuid,'22222222-2222-2222-2222-222222222222','aaaaaaaa-0000-0000-0000-000000000000'),
  ('v2','0a222222-2222-2222-2222-222222222222'::uuid,'22222222-2222-2222-2222-222222222222','aaaaaaaa-0000-0000-0000-000000000000'),
  ('v3','0a333333-3333-3333-3333-333333333333'::uuid,'22222222-2222-2222-2222-222222222222','aaaaaaaa-0000-0000-0000-000000000000');
INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system) VALUES
  -- a system relation that must NOT count toward structure
  ('0b111111-1111-1111-1111-111111111111'::uuid,'e0000000-0000-0000-0000-000000000001'::uuid,
   '99999999-9999-9999-9999-999999999999','22222222-2222-2222-2222-222222222222',
   'ffffffff-0000-0000-0000-000000000001','aaaaaaaa-0000-0000-0000-000000000000',true),
  -- TYPES relation assigning a boosted type to entity 5
  ('0b222222-2222-2222-2222-222222222222'::uuid,'e0000000-0000-0000-0000-000000000002'::uuid,
   '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','55555555-5555-5555-5555-555555555555',
   'cccccccc-0000-0000-0000-000000000001','aaaaaaaa-0000-0000-0000-000000000000',false);
INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system) VALUES
  ('0b333333-3333-3333-3333-333333333333'::uuid,'e0000000-0000-0000-0000-000000000003'::uuid,
   '99999999-9999-9999-9999-999999999999','66666666-6666-6666-6666-666666666666',
   'ffffffff-0000-0000-0000-000000000002','aaaaaaaa-0000-0000-0000-000000000000',true),
  ('0b444444-4444-4444-4444-444444444444'::uuid,'e0000000-0000-0000-0000-000000000004'::uuid,
   '99999999-9999-9999-9999-999999999999','66666666-6666-6666-6666-666666666666',
   'ffffffff-0000-0000-0000-000000000003','aaaaaaaa-0000-0000-0000-000000000000',true);
INSERT INTO entity_type_weights (type_id, weight, note)
  VALUES ('cccccccc-0000-0000-0000-000000000001', 3.0, 'News story');

SELECT assert(refresh_entity_ranking_scores(ARRAY[
  '11111111-1111-1111-1111-111111111111','22222222-2222-2222-2222-222222222222',
  '33333333-3333-3333-3333-333333333333','44444444-4444-4444-4444-444444444444',
  '55555555-5555-5555-5555-555555555555','66666666-6666-6666-6666-666666666666']::uuid[]) = 6, 'refresh scored all 6 entities');

-- ---- end-to-end behaviour ----
SELECT assert((SELECT ranking_score FROM entity_ranking_scores WHERE entity_id='11111111-1111-1111-1111-111111111111')
            > (SELECT ranking_score FROM entity_ranking_scores WHERE entity_id='33333333-3333-3333-3333-333333333333'),
  'same quality, newer entity ranks higher (recency term works and has the right SIGN)');
SELECT assert((SELECT ranking_score FROM entity_ranking_scores WHERE entity_id='11111111-1111-1111-1111-111111111111')
            > (SELECT ranking_score FROM entity_ranking_scores WHERE entity_id='44444444-4444-4444-4444-444444444444'),
  'same age, upvoted beats downvoted');
SELECT assert((SELECT positive FROM entity_ranking_scores WHERE entity_id='22222222-2222-2222-2222-222222222222') = 0,
  'vote_kind=1 (stance) is excluded from the feed quality term');
SELECT assert((SELECT intrinsic_score FROM entity_ranking_scores WHERE entity_id='22222222-2222-2222-2222-222222222222') > 0,
  'unvoted-but-rich entity gets a non-zero intrinsic score — the only gradient in the sparse regime');
SELECT assert((SELECT intrinsic_score FROM entity_ranking_scores WHERE entity_id='11111111-1111-1111-1111-111111111111') = 0,
  'entity with no values/relations gets 0 intrinsic (system relation did not count)');
SELECT assert((SELECT intrinsic_score FROM entity_ranking_scores WHERE entity_id='66666666-6666-6666-6666-666666666666') = 0,
  'an entity whose ONLY relations are is_system gets 0 intrinsic — plumbing must not inflate structural quality');
SELECT assert((SELECT type_weight FROM entity_ranking_scores WHERE entity_id='55555555-5555-5555-5555-555555555555') = 3.0,
  'type weight resolved via the TYPES relation');
SELECT assert((SELECT type_weight FROM entity_ranking_scores WHERE entity_id='11111111-1111-1111-1111-111111111111') = 1.0,
  'untyped entity defaults to weight 1.0');
-- idempotency
SELECT assert(refresh_entity_ranking_scores(ARRAY['11111111-1111-1111-1111-111111111111']::uuid[]) = 1, 're-refresh is an upsert, not a duplicate');
SELECT assert((SELECT count(*) FROM entity_ranking_scores) = 6, 'still exactly 6 rows after re-refresh');
SELECT assert((SELECT count(DISTINCT ranking_score) FROM entity_ranking_scores) = 6,
  'all distinct-by-design fixtures get DISTINCT scores — no degenerate ties');
\echo '=== ALL ASSERTIONS PASSED ==='
