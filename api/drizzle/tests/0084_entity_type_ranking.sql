-- Assertions for 0084: entity_type_ranking is a RECONCILE, not an append.
--
-- Run per drizzle/tests/README.md. Truncates its own fixtures so it passes in any order.
--
-- What is actually at risk here is not "does it populate" — any implementation populates, and
-- every populate-shaped assertion passes under all the wrong ones too. The four real risks:
--
--   1. IT NEVER DELETES. An append-only sync leaves an entity listed under a type it no longer
--      has, so the typed feed serves it forever. Invisible to every "the rows are there" check.
--   2. THE DELETE IS UNSCOPED. The opposite mistake: a reconcile that clears more than the ids
--      it was handed silently empties the table for everyone else on the next vote.
--   3. THE SCORE FREEZES. Drop `DO UPDATE` from the upsert and the table keeps whatever score
--      it first saw. Ordering then drifts from entity_ranking_scores with nothing failing.
--   4. THE MULTI-TYPE QUERY LOSES ITS ORDERING. `= ANY` over several types reads every match
--      and top-N sorts; only the per-type LATERAL walks the index. Both return the same rows,
--      so only a plan assertion tells them apart — see the last section.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN IF NOT cond THEN RAISE EXCEPTION 'FAIL: %', label; ELSE RAISE NOTICE 'pass: %', label; END IF; END; $$;

TRUNCATE entities, values, relations, votes_count, entity_ranking_scores,
         entity_type_weights, entity_type_exclusions, entity_type_ranking;

UPDATE entity_ranking_config
   SET participation_weight = 7, participation_cap = 30,
       comment_weight = 0, comment_cap = 30, tau_seconds = 100000
 WHERE id;

-- ---------------------------------------------------------------------------
-- Fixtures.
--   e1  two types (t1, t2)      -- the multi-type case
--   e2  one type  (t1)
--   e3  one type  (t1), declared TWICE -- the duplicate-relation case
--   e4  one type  (t1)
--   e5  one type  (t1), declared twice exactly like e3 -- e3's tie partner
--
-- e3/e5 are the tie pair rather than e3/e4: `intrinsic_score` counts an entity's relations, and
-- e3 carries two Types rows to e4's one, so those two never tie. The first version of this file
-- paired them and the tie assertion caught it — which is the reason that assertion exists.
-- ---------------------------------------------------------------------------
INSERT INTO entities (id, created_at) VALUES
  ('e0000000-0000-0000-0000-000000000001','1785478598'),
  ('e0000000-0000-0000-0000-000000000002','1785478598'),
  ('e0000000-0000-0000-0000-000000000003','1785478598'),
  ('e0000000-0000-0000-0000-000000000004','1785478598'),
  ('e0000000-0000-0000-0000-000000000005','1785478598'),
  ('70000000-0000-0000-0000-000000000001','1785478598'),
  ('70000000-0000-0000-0000-000000000002','1785478598');

-- Every fixture needs a name, or candidate generation drops it (0075).
INSERT INTO values (id, entity_id, property_id, space_id, text)
SELECT gen_random_uuid(), e.id, 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid,
       'bbbbbbbb-0000-0000-0000-000000000001'::uuid, 'fixture'
FROM entities e;

INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system) VALUES
  ('0b000001-0000-0000-0000-000000000001','ef000000-0000-0000-0000-000000000001','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e0000000-0000-0000-0000-000000000001','70000000-0000-0000-0000-000000000001','bbbbbbbb-0000-0000-0000-000000000001',false),
  ('0b000002-0000-0000-0000-000000000002','ef000000-0000-0000-0000-000000000002','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e0000000-0000-0000-0000-000000000001','70000000-0000-0000-0000-000000000002','bbbbbbbb-0000-0000-0000-000000000001',false),
  ('0b000003-0000-0000-0000-000000000003','ef000000-0000-0000-0000-000000000003','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e0000000-0000-0000-0000-000000000002','70000000-0000-0000-0000-000000000001','bbbbbbbb-0000-0000-0000-000000000001',false),
  -- e3 declares the same type twice, from two spaces — the real shape of a duplicate. Without
  -- DISTINCT the upsert raises "ON CONFLICT DO UPDATE command cannot affect row a second time".
  ('0b000004-0000-0000-0000-000000000004','ef000000-0000-0000-0000-000000000004','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e0000000-0000-0000-0000-000000000003','70000000-0000-0000-0000-000000000001','bbbbbbbb-0000-0000-0000-000000000001',false),
  ('0b000005-0000-0000-0000-000000000005','ef000000-0000-0000-0000-000000000005','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e0000000-0000-0000-0000-000000000003','70000000-0000-0000-0000-000000000001','cccccccc-0000-0000-0000-000000000001',false),
  ('0b000006-0000-0000-0000-000000000006','ef000000-0000-0000-0000-000000000006','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e0000000-0000-0000-0000-000000000004','70000000-0000-0000-0000-000000000001','bbbbbbbb-0000-0000-0000-000000000001',false),
  -- e5 mirrors e3 exactly, down to the duplicate, so the two are identical by construction.
  ('0b000008-0000-0000-0000-000000000008','ef000000-0000-0000-0000-000000000008','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e0000000-0000-0000-0000-000000000005','70000000-0000-0000-0000-000000000001','bbbbbbbb-0000-0000-0000-000000000001',false),
  ('0b000009-0000-0000-0000-000000000009','ef000000-0000-0000-0000-000000000009','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e0000000-0000-0000-0000-000000000005','70000000-0000-0000-0000-000000000001','cccccccc-0000-0000-0000-000000000001',false);

SELECT public.refresh_entity_ranking_scores(ARRAY[
  'e0000000-0000-0000-0000-000000000001','e0000000-0000-0000-0000-000000000002',
  'e0000000-0000-0000-0000-000000000003','e0000000-0000-0000-0000-000000000004',
  'e0000000-0000-0000-0000-000000000005']::uuid[]);

-- ---- population ------------------------------------------------------------
SELECT assert((SELECT count(*) FROM entity_type_ranking) = 6,
  'one row per (entity, distinct type): e1 x2 + e2 + e3 + e4 + e5');

SELECT assert((SELECT count(*) FROM entity_type_ranking
                WHERE entity_id = 'e0000000-0000-0000-0000-000000000001') = 2,
  'a multi-type entity is listed under BOTH its types');

SELECT assert((SELECT count(*) FROM entity_type_ranking
                WHERE entity_id = 'e0000000-0000-0000-0000-000000000003') = 1,
  'a type declared twice produces one row, not a duplicate-key error');

SELECT assert((SELECT count(*) FROM entity_type_ranking etr
                JOIN entity_ranking_scores ers ON ers.entity_id = etr.entity_id
               WHERE etr.ranking_score <> ers.ranking_score) = 0,
  'every denormalised score equals the score table it was copied from');

-- ---- risk 3: the score must follow a re-score ------------------------------
INSERT INTO votes_count (object_id, object_type, space_id, vote_kind, positive, negative)
VALUES ('e0000000-0000-0000-0000-000000000002', 0, 'bbbbbbbb-0000-0000-0000-000000000001', 0, 40, 0);

SELECT public.refresh_entity_ranking_scores(ARRAY['e0000000-0000-0000-0000-000000000002']::uuid[]);

SELECT assert((SELECT etr.ranking_score FROM entity_type_ranking etr
                WHERE etr.entity_id = 'e0000000-0000-0000-0000-000000000002')
            = (SELECT ers.ranking_score FROM entity_ranking_scores ers
                WHERE ers.entity_id = 'e0000000-0000-0000-0000-000000000002'),
  'a re-score updates the denormalised copy (upsert is DO UPDATE, not DO NOTHING)');

SELECT assert((SELECT etr.ranking_score FROM entity_type_ranking etr
                WHERE etr.entity_id = 'e0000000-0000-0000-0000-000000000002')
            > (SELECT etr.ranking_score FROM entity_type_ranking etr
                WHERE etr.entity_id = 'e0000000-0000-0000-0000-000000000004'),
  'and the update actually moved it — 40 curation votes now outrank the tie partner');

-- ---- risk 1: losing a type must remove the row -----------------------------
DELETE FROM relations
 WHERE from_entity_id = 'e0000000-0000-0000-0000-000000000001'
   AND to_entity_id   = '70000000-0000-0000-0000-000000000002';

SELECT public.refresh_entity_ranking_scores(ARRAY['e0000000-0000-0000-0000-000000000001']::uuid[]);

SELECT assert((SELECT count(*) FROM entity_type_ranking
                WHERE entity_id = 'e0000000-0000-0000-0000-000000000001'
                  AND type_id   = '70000000-0000-0000-0000-000000000002') = 0,
  'a removed Types relation removes the row (the sync reconciles, it does not append)');

SELECT assert((SELECT count(*) FROM entity_type_ranking
                WHERE entity_id = 'e0000000-0000-0000-0000-000000000001') = 1,
  'and the type it still has is untouched');

-- ---- risk 2: the reconcile must not reach past its arguments ---------------
-- Counting rows after refreshing e1 is NOT enough, and the first version of this file made
-- exactly that mistake: every other fixture's Types relations still existed, so an unscoped
-- DELETE would have removed nothing and the assertion passed under the mutation it claimed to
-- cover. It needs a row that an unscoped reconcile WOULD sweep.
--
-- Plant one: e4 listed under t2, with no Types relation to back it. That is the real residual
-- state 0084's header describes — a type was removed and nothing has re-scored that entity yet.
INSERT INTO entity_type_ranking (type_id, entity_id, ranking_score)
VALUES ('70000000-0000-0000-0000-000000000002','e0000000-0000-0000-0000-000000000004', 0.5);

SELECT public.refresh_entity_ranking_scores(ARRAY['e0000000-0000-0000-0000-000000000001']::uuid[]);

SELECT assert((SELECT count(*) FROM entity_type_ranking
                WHERE entity_id = 'e0000000-0000-0000-0000-000000000004'
                  AND type_id   = '70000000-0000-0000-0000-000000000002') = 1,
  'refreshing e1 leaves another entity''s stale row alone — the DELETE is scoped to its ids');

-- ...and the survival above is because of the scoping, not because the DELETE is broken: hand
-- the function that entity and the same row goes.
SELECT public.refresh_entity_ranking_scores(ARRAY['e0000000-0000-0000-0000-000000000004']::uuid[]);

SELECT assert((SELECT count(*) FROM entity_type_ranking
                WHERE entity_id = 'e0000000-0000-0000-0000-000000000004'
                  AND type_id   = '70000000-0000-0000-0000-000000000002') = 0,
  'and refreshing THAT entity does remove it, so the assertion above is not vacuous');

SELECT assert((SELECT count(*) FROM entity_type_ranking) = 5,
  'the table is back to one row per real (entity, type) pair');

-- ---- gaining a type --------------------------------------------------------
INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system) VALUES
  ('0b000007-0000-0000-0000-000000000007','ef000000-0000-0000-0000-000000000007','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e0000000-0000-0000-0000-000000000002','70000000-0000-0000-0000-000000000002','bbbbbbbb-0000-0000-0000-000000000001',false);

SELECT public.refresh_entity_ranking_scores(ARRAY['e0000000-0000-0000-0000-000000000002']::uuid[]);

SELECT assert((SELECT count(*) FROM entity_type_ranking
                WHERE entity_id = 'e0000000-0000-0000-0000-000000000002') = 2,
  'a newly added Types relation appears on the next re-score');

-- ---- the sort key is a strict total order ----------------------------------
-- e3 and e5 are identical by construction: same created_at, no votes, the same two relations.
-- Without `entity_id DESC` the pair has no defined order and cursor pagination can repeat or
-- skip a row.
SELECT assert((SELECT etr.ranking_score FROM entity_type_ranking etr
                WHERE etr.entity_id = 'e0000000-0000-0000-0000-000000000003')
            = (SELECT etr.ranking_score FROM entity_type_ranking etr
                WHERE etr.entity_id = 'e0000000-0000-0000-0000-000000000005'),
  'the tie fixtures really do tie on score, so the next assertion is not vacuous');

SELECT assert((SELECT etr.entity_id FROM entity_type_ranking etr
                WHERE etr.type_id = '70000000-0000-0000-0000-000000000001'
                  AND etr.entity_id IN ('e0000000-0000-0000-0000-000000000003',
                                        'e0000000-0000-0000-0000-000000000005')
                ORDER BY etr.ranking_score DESC, etr.entity_id DESC LIMIT 1)
            = 'e0000000-0000-0000-0000-000000000005',
  'ties break on entity_id DESC, matching entity_ranking_scores_ranking_desc_idx');

-- ---- risk 4: the multi-type query shape ------------------------------------
-- Correctness only — the LATERAL and the `= ANY` phrasing return the SAME rows, which is
-- exactly why this trap survives review. The difference is 3.3ms vs 251ms on live data and is
-- only visible in EXPLAIN, so it is pinned in 0084's header rather than here. What this does
-- pin is that the LATERAL shape is not subtly wrong: per-type walks, merged, still yield a
-- single correctly ordered result.
SELECT assert((
  SELECT array_agg(s.entity_id ORDER BY s.ranking_score DESC, s.entity_id DESC)
  FROM unnest(ARRAY['70000000-0000-0000-0000-000000000001',
                    '70000000-0000-0000-0000-000000000002']::uuid[]) AS t(tid)
  CROSS JOIN LATERAL (
    SELECT etr.entity_id, etr.ranking_score
    FROM entity_type_ranking etr
    WHERE etr.type_id = t.tid
    ORDER BY etr.ranking_score DESC, etr.entity_id DESC
    LIMIT 10
  ) s
) = (
  SELECT array_agg(etr.entity_id ORDER BY etr.ranking_score DESC, etr.entity_id DESC)
  FROM entity_type_ranking etr
  WHERE etr.type_id = ANY(ARRAY['70000000-0000-0000-0000-000000000001',
                                '70000000-0000-0000-0000-000000000002']::uuid[])
), 'the per-type LATERAL returns the same ordered rows as the naive = ANY phrasing');


-- ---- THE DUPLICATE HAZARD, for whoever writes the switch --------------------
-- `entities_ranked_for_feed_by_type` de-duplicates for free today: `e.id IN (SELECT ...)` is a
-- semi-join, so an entity carrying two of the requested types is returned once. The LATERAL is
-- NOT a semi-join — it is one ordered walk per type, concatenated — so that entity comes back
-- once per matching type. With Explore defaulting to News story + Debate + Claim, anything
-- typed both Claim and News story would render as two cards.
--
-- Pinned here so the switch cannot land without handling it.
SELECT assert((
  SELECT count(*) FROM unnest(ARRAY['70000000-0000-0000-0000-000000000001',
                                    '70000000-0000-0000-0000-000000000002']::uuid[]) AS t(tid)
  CROSS JOIN LATERAL (
    SELECT etr.entity_id FROM entity_type_ranking etr
    WHERE etr.type_id = t.tid ORDER BY etr.ranking_score DESC, etr.entity_id DESC LIMIT 10
  ) s
  WHERE s.entity_id = 'e0000000-0000-0000-0000-000000000002'
) = 2, 'the bare LATERAL returns a two-typed entity TWICE — the switch must de-duplicate');

-- And the de-duplicating shape it needs: DISTINCT ON the entity, then re-sort. The score is the
-- same whichever type row wins, so which one DISTINCT ON keeps does not matter.
SELECT assert((
  SELECT array_agg(d.entity_id ORDER BY d.ranking_score DESC, d.entity_id DESC)
  FROM (
    SELECT DISTINCT ON (s.entity_id) s.entity_id, s.ranking_score
    FROM unnest(ARRAY['70000000-0000-0000-0000-000000000001',
                      '70000000-0000-0000-0000-000000000002']::uuid[]) AS t(tid)
    CROSS JOIN LATERAL (
      SELECT etr.entity_id, etr.ranking_score FROM entity_type_ranking etr
      WHERE etr.type_id = t.tid ORDER BY etr.ranking_score DESC, etr.entity_id DESC LIMIT 10
    ) s
    ORDER BY s.entity_id, s.ranking_score DESC
  ) d
) = ARRAY['e0000000-0000-0000-0000-000000000002',
          'e0000000-0000-0000-0000-000000000005',
          'e0000000-0000-0000-0000-000000000003',
          'e0000000-0000-0000-0000-000000000004',
          'e0000000-0000-0000-0000-000000000001']::uuid[],
  'DISTINCT ON (entity_id) removes the duplicate and keeps the ranked order intact');

DROP FUNCTION assert(boolean, text);
