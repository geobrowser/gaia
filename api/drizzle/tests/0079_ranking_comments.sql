-- Assertions for 0079: comments feed the ranking score, counted by distinct commenter.
--
-- Run per drizzle/tests/README.md. Truncates its own fixtures so it passes in any order.
--
-- Two risks are load-bearing here, and neither is "does the arithmetic work":
--
--   1. The term is too small to change an ordering. Same failure 0078 was written against
--      — every value assertion green, feed unchanged — so the central assertion compares
--      an ORDERING across a realistic age gap.
--   2. It counts raw comments instead of distinct commenters. That one is invisible in
--      any test where every comment comes from a different space, which is what a naive
--      fixture looks like. So one fixture here is deliberately forty comments from one
--      account.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN IF NOT cond THEN RAISE EXCEPTION 'FAIL: %', label; ELSE RAISE NOTICE 'pass: %', label; END IF; END; $$;

TRUNCATE entities, values, relations, votes_count, entity_ranking_scores,
         entity_type_weights, entity_type_exclusions;

UPDATE entity_ranking_config
   SET participation_weight = 0, participation_cap = 10,
       comment_weight = 0, comment_cap = 30, tau_seconds = 45000
 WHERE id;

-- ---------------------------------------------------------------------------
-- Ships inert. Read from the catalog, not the row: this suite rewrites the row
-- above, so a row assertion would only be checking its own UPDATE.
-- ---------------------------------------------------------------------------
SELECT assert((SELECT column_default::numeric FROM information_schema.columns
                WHERE table_name = 'entity_ranking_config'
                  AND column_name = 'comment_weight') = 0,
  'comment_weight column DEFAULT is 0 — applying this migration must not arm the term');
SELECT assert((SELECT column_default::numeric FROM information_schema.columns
                WHERE table_name = 'entity_ranking_scores'
                  AND column_name = 'comment_score') = 0,
  'comment_score column DEFAULT is 0 for rows written before a rescore');

-- ---------------------------------------------------------------------------
-- The 8-argument score must be the one that exists. If the 7-argument version
-- survived, `refresh_entity_ranking_scores` would bind to it and drop the comment
-- term silently — no error, no effect.
-- ---------------------------------------------------------------------------
SELECT assert((SELECT count(*) FROM pg_proc p
                JOIN pg_namespace n ON n.oid = p.pronamespace
               WHERE n.nspname = 'public' AND p.proname = 'entity_ranking_score'
                 AND p.pronargs = 8) = 1,
  'the 8-argument entity_ranking_score exists');
SELECT assert((SELECT count(*) FROM pg_proc p
                JOIN pg_namespace n ON n.oid = p.pronamespace
               WHERE n.nspname = 'public' AND p.proname = 'entity_ranking_score'
                 AND p.pronargs = 7) = 0,
  'the 7-argument entity_ranking_score is GONE — leaving it means silent binding to the stale term');

-- ---------------------------------------------------------------------------
-- Fixtures. Same 13.55-day gap as 0078, so the comparison is against a real
-- recency advantage rather than a contrived one.
--   OLD_COMMENTED = 5 distinct commenters, older
--   NEW_QUIET     = no comments, newer
--   BRIGADED      = 40 comments from ONE space, newer
-- ---------------------------------------------------------------------------
INSERT INTO entities (id, created_at) VALUES
  ('c0000000-0000-0000-0000-000000000001','1785478598'),  -- old, 5 commenters
  ('c0000000-0000-0000-0000-000000000002','1786649398'),  -- new, no comments
  ('c0000000-0000-0000-0000-000000000003','1786649398');  -- new, 40 comments from one space

-- 5 distinct commenters on the old entity.
INSERT INTO relations (id, entity_id, space_id, type_id, from_entity_id, to_entity_id, is_system)
SELECT gen_random_uuid(),
       gen_random_uuid(),
       ('bbbbbbbb-0000-0000-0000-00000000000' || i)::uuid,
       '310d4a24-0e5b-451c-b215-1bfce40d0fe6'::uuid,
       gen_random_uuid(),
       'c0000000-0000-0000-0000-000000000001'::uuid,
       false
FROM generate_series(1, 5) AS i;

-- 40 comments, all authored from the same space. Distinct commenters = 1.
INSERT INTO relations (id, entity_id, space_id, type_id, from_entity_id, to_entity_id, is_system)
SELECT gen_random_uuid(),
       gen_random_uuid(),
       'bbbbbbbb-0000-0000-0000-0000000000ff'::uuid,
       '310d4a24-0e5b-451c-b215-1bfce40d0fe6'::uuid,
       gen_random_uuid(),
       'c0000000-0000-0000-0000-000000000003'::uuid,
       false
FROM generate_series(1, 40) AS i;

SELECT assert(refresh_entity_ranking_scores(ARRAY[
  'c0000000-0000-0000-0000-000000000001','c0000000-0000-0000-0000-000000000002',
  'c0000000-0000-0000-0000-000000000003']::uuid[]) = 3,
  'refresh scored all 3 entities');

-- ---------------------------------------------------------------------------
-- Counting: distinct commenters, not comments.
-- ---------------------------------------------------------------------------
SELECT assert((SELECT commenter_count FROM entity_ranking_scores
                WHERE entity_id = 'c0000000-0000-0000-0000-000000000001') = 5,
  'five separate accounts count as five commenters');
SELECT assert((SELECT commenter_count FROM entity_ranking_scores
                WHERE entity_id = 'c0000000-0000-0000-0000-000000000003') = 1,
  'forty comments from one account count as ONE commenter — the whole point of DISTINCT space_id');
SELECT assert((SELECT commenter_count FROM entity_ranking_scores
                WHERE entity_id = 'c0000000-0000-0000-0000-000000000002') = 0,
  'an entity nobody replied to has no commenters');

-- Unarmed, the term must be exactly 0 and must not have moved anything.
SELECT assert((SELECT comment_score FROM entity_ranking_scores
                WHERE entity_id = 'c0000000-0000-0000-0000-000000000001') = 0,
  'with comment_weight 0 the term is 0 even for a well-commented entity');
SELECT assert((SELECT ranking_score FROM entity_ranking_scores
                WHERE entity_id = 'c0000000-0000-0000-0000-000000000002')
            > (SELECT ranking_score FROM entity_ranking_scores
                WHERE entity_id = 'c0000000-0000-0000-0000-000000000001'),
  'unarmed, the newer quiet entity still wins on recency — this migration changes no ordering');

-- ---------------------------------------------------------------------------
-- Armed. This is the assertion that would fail if the term were merely present
-- but too small to matter.
-- ---------------------------------------------------------------------------
-- tau 100000, matching 0078's armed section and the armed production config. At the
-- shipped default of 45000 a 13.55-day gap is worth 26.0 score units and no capped
-- engagement term can cross it — which is a fact about tau, not about this term, and
-- asserting against 45000 would just be testing the wrong config.
UPDATE entity_ranking_config
   SET comment_weight = 7, comment_cap = 30, tau_seconds = 100000
 WHERE id;
SELECT refresh_entity_ranking_scores(ARRAY[
  'c0000000-0000-0000-0000-000000000001','c0000000-0000-0000-0000-000000000002',
  'c0000000-0000-0000-0000-000000000003']::uuid[]);

SELECT assert((SELECT ranking_score FROM entity_ranking_scores
                WHERE entity_id = 'c0000000-0000-0000-0000-000000000001')
            > (SELECT ranking_score FROM entity_ranking_scores
                WHERE entity_id = 'c0000000-0000-0000-0000-000000000002'),
  'armed, the 13.55-day-older entity with 5 commenters now outranks the newer quiet one');

-- Stated on the term, not on the total. The brigaded entity is also 13.55 days newer, so
-- comparing totals would conflate the guarantee with recency and pass or fail on tau.
SELECT assert((SELECT comment_score FROM entity_ranking_scores
                WHERE entity_id = 'c0000000-0000-0000-0000-000000000003')
            < (SELECT comment_score FROM entity_ranking_scores
                WHERE entity_id = 'c0000000-0000-0000-0000-000000000001'),
  'forty comments from one account earn a smaller term than five from five accounts');
SELECT assert(abs((SELECT comment_score FROM entity_ranking_scores
                    WHERE entity_id = 'c0000000-0000-0000-0000-000000000003')
                  - 7 * ln(2)) < 0.0001,
  'the brigaded entity is scored as ONE commenter (7*ln(2)), not forty (7*ln(41))');

-- The cap is the brigading guardrail: it has to bind before the term can buy
-- unbounded recency.
SELECT assert(public.entity_participation_score(1000000, 7, 30) = 30,
  'the comment cap binds however many commenters arrive');

UPDATE entity_ranking_config
   SET comment_weight = 0, comment_cap = 30, tau_seconds = 45000
 WHERE id;
