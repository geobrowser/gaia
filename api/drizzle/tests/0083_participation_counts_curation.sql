-- Assertions for 0083: curation votes count as participation, not quality alone.
--
-- Run per drizzle/tests/README.md. Truncates its own fixtures so it passes in any order.
--
-- Three risks are load-bearing, and none of them is "does the arithmetic work":
--
--   1. The term is too small to change an ordering. Same failure 0078 and 0079 were
--      written against — every value assertion green, feed unchanged. So the central
--      assertion compares an ORDERING across a real recency gap, which is the thing
--      the Explore feed actually consumes.
--   2. Curation gets MOVED rather than ADDED. A plausible wrong implementation drops
--      curation from the Wilson term while wiring it into participation. That still
--      makes debates rank, so every debate-shaped assertion passes — and quality
--      silently stops meaning anything. So quality is asserted to still read curation.
--   3. Stance regresses. Curation and stance must be additive; an entity carrying both
--      must beat one carrying either alone.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN IF NOT cond THEN RAISE EXCEPTION 'FAIL: %', label; ELSE RAISE NOTICE 'pass: %', label; END IF; END; $$;

TRUNCATE entities, values, relations, votes_count, entity_ranking_scores,
         entity_type_weights, entity_type_exclusions CASCADE;

-- Arm participation, and use the live tau so the recency gap below is realistic.
UPDATE entity_ranking_config
   SET participation_weight = 7, participation_cap = 30,
       comment_weight = 0, comment_cap = 30, tau_seconds = 100000
 WHERE id;

-- ---------------------------------------------------------------------------
-- Fixtures. The 13.55-day gap 0078 and 0079 both use, so the comparison is against
-- a real recency advantage rather than a contrived one.
--   OLD_CURATED  = 20 curation votes, older      (the Debate shape)
--   NEW_QUIET    = no votes at all, newer        (the Claim-with-nothing shape)
--   OLD_STANCED  = 20 stance votes, older        (the Claim shape — must not regress)
--   OLD_BOTH     = 10 curation + 10 stance, older
-- ---------------------------------------------------------------------------
-- The real `entities` table has created_at_block, updated_at and updated_at_block NOT NULL.
-- The block columns are placeholders: nothing in the scoring path reads them.
INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
SELECT v.id::uuid, v.created_at::text, '0', v.created_at::text, '0' FROM (VALUES
  ('d0000000-0000-0000-0000-000000000001','1785478598'),
  ('d0000000-0000-0000-0000-000000000002','1786649398'),
  ('d0000000-0000-0000-0000-000000000003','1785478598'),
  ('d0000000-0000-0000-0000-000000000004','1785478598')
) AS v(id, created_at);

-- Every fixture needs a name, or candidate generation drops it (0075).
INSERT INTO values (id, entity_id, property_id, space_id, text)
SELECT gen_random_uuid(), e.id, 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid,
       'bbbbbbbb-0000-0000-0000-000000000001'::uuid, 'fixture'
  FROM entities e;

INSERT INTO votes_count (object_id, object_type, space_id, vote_kind, positive, negative) VALUES
  ('d0000000-0000-0000-0000-000000000001', 0, 'bbbbbbbb-0000-0000-0000-000000000001', 0, 15, 5),
  ('d0000000-0000-0000-0000-000000000003', 0, 'bbbbbbbb-0000-0000-0000-000000000001', 1, 15, 5),
  ('d0000000-0000-0000-0000-000000000004', 0, 'bbbbbbbb-0000-0000-0000-000000000001', 0,  8, 2),
  ('d0000000-0000-0000-0000-000000000004', 0, 'bbbbbbbb-0000-0000-0000-000000000001', 1,  8, 2);

SELECT public.refresh_entity_ranking_scores(ARRAY(SELECT id FROM entities));

-- ---------------------------------------------------------------------------
-- 1. Curation now produces participation. This is the whole defect: before 0083
--    an entity whose only axis is curation scored exactly 0 here.
-- ---------------------------------------------------------------------------
SELECT assert((SELECT participation_score FROM entity_ranking_scores
                WHERE entity_id='d0000000-0000-0000-0000-000000000001') > 0,
  'curation-only entity has participation > 0');

-- 2. Stance still produces participation — 0078 must not regress.
SELECT assert((SELECT participation_score FROM entity_ranking_scores
                WHERE entity_id='d0000000-0000-0000-0000-000000000003') > 0,
  'stance-only entity still has participation > 0');

-- 3. Equal volume on either axis scores equally. Participation measures volume, so
--    which axis carried it must not matter.
SELECT assert((SELECT participation_score FROM entity_ranking_scores
                WHERE entity_id='d0000000-0000-0000-0000-000000000001')
            = (SELECT participation_score FROM entity_ranking_scores
                WHERE entity_id='d0000000-0000-0000-0000-000000000003'),
  '20 curation votes and 20 stance votes give the same participation');

-- 4. The axes are ADDITIVE, not either/or.
SELECT assert((SELECT participation_score FROM entity_ranking_scores
                WHERE entity_id='d0000000-0000-0000-0000-000000000004')
            = (SELECT participation_score FROM entity_ranking_scores
                WHERE entity_id='d0000000-0000-0000-0000-000000000001'),
  '10 curation + 10 stance equals 20 on one axis — the axes sum');

-- ---------------------------------------------------------------------------
-- 5. Curation is ADDED to participation, not MOVED off quality. Without this a
--    wrong implementation that relocates the signal passes everything above.
-- ---------------------------------------------------------------------------
SELECT assert((SELECT quality_score FROM entity_ranking_scores
                WHERE entity_id='d0000000-0000-0000-0000-000000000001')
            > (SELECT quality_score FROM entity_ranking_scores
                WHERE entity_id='d0000000-0000-0000-0000-000000000002'),
  'curation still raises the Wilson quality term — it was added, not moved');

SELECT assert((SELECT positive FROM entity_ranking_scores
                WHERE entity_id='d0000000-0000-0000-0000-000000000001') = 15,
  'curation tallies are still recorded on the score row');

-- ---------------------------------------------------------------------------
-- 6. THE ONE THAT MATTERS. An older curated entity must now outrank a newer
--    silent one across a 13.55-day gap. Every assertion above can pass while the
--    feed is unchanged; this is the one the Explore page actually reflects.
-- ---------------------------------------------------------------------------
SELECT assert((SELECT ranking_score FROM entity_ranking_scores
                WHERE entity_id='d0000000-0000-0000-0000-000000000001')
            > (SELECT ranking_score FROM entity_ranking_scores
                WHERE entity_id='d0000000-0000-0000-0000-000000000002'),
  'an older curation-engaged entity outranks a newer silent one — the feed actually moves');

TRUNCATE entities, values, relations, votes_count, entity_ranking_scores,
         entity_type_weights, entity_type_exclusions CASCADE;
