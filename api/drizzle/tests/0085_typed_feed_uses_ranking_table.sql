-- Assertions for 0085: the typed feed reads entity_type_ranking.
--
-- Run per drizzle/tests/README.md. Truncates its own fixtures so it passes in any order.
--
-- The equivalence this migration rests on was proven against LIVE data, not here: old and
-- new return identical id sets in identical order for Debate, Person, News story, Explore's
-- three defaults, Claim (334,808 members) and the 12-type whitelist — 0 rows exclusive to
-- either side and 0 positional differences in all six. What this file pins is the set of
-- ways the rewrite could go wrong that a same-rows check would NOT catch:
--
--   1. DE-DUPLICATION. The function it replaces used `id IN (SELECT ...)`, a semi-join,
--      which returns a two-typed entity once. A LATERAL is one walk per type concatenated
--      and returns it twice. Explore opens with three types ticked, so this is reachable.
--   2. THE CAP TRUNCATING RESULTS. `max_per_type` is per type, applied before the global
--      ordering. Too small and a deep page silently returns short.
--   3. PREDICATES ESCAPING THE LATERAL. Applied outside, the per-type cap counts rows that
--      are then filtered away, so the feed returns fewer rows than asked for.
--   4. THE OVERLOAD. `CREATE OR REPLACE` cannot change an argument list; without the DROP
--      both signatures exist and every call fails "is not unique".
--
-- Calls below use NAMED argument notation (`max_per_type => 1`). Writing them positionally
-- is how the first draft of this file passed `1` as `space_ids`; every argument after
-- `type_ids` is optional and several are nullable, so a positional slip typechecks and then
-- silently tests the wrong thing.

\set ON_ERROR_STOP on

-- `IS NOT TRUE`, not `NOT cond`. Under `NOT cond` a NULL condition is neither true nor
-- false, so it takes the ELSE branch and reports a PASS — the vacuous-assertion trap in
-- README.md. A `SELECT ... INTO` that matches no row leaves its variable NULL, and the
-- assertion below it then proves nothing while printing "pass".
CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

TRUNCATE entities, values, relations, votes_count, entity_ranking_scores,
         entity_type_weights, entity_type_exclusions, entity_type_ranking, entity_feed_blocklist CASCADE;

UPDATE entity_ranking_config
   SET participation_weight = 7, participation_cap = 30,
       comment_weight = 0, comment_cap = 30, tau_seconds = 100000
 WHERE id;

-- ---------------------------------------------------------------------------
-- Fixtures. tA and tB are two types a caller might request together.
--   e1  BOTH types           -- the duplicate case
--   e2  tA only
--   e3  tB only
--   e4  tA only, BLOCKLISTED -- must never be served
--   e5  tA only, no name row at all
--   e6  tA only, name row present but BLANK -- the case a missing-row fixture cannot test
--
-- e2 and e3 carry curation votes so they outscore e1. That is not decoration: e1 is the
-- entity in BOTH types, and if it topped both, a per-type cap of 1 and a global cap of 1
-- would return the same single row and the cap assertion below could not tell them apart.
-- ---------------------------------------------------------------------------
-- The real `entities` table has created_at_block, updated_at and updated_at_block NOT NULL.
-- The block columns are placeholders: nothing in the scoring path reads them.
INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
SELECT v.id::uuid, v.created_at::text, '0', v.created_at::text, '0' FROM (VALUES
  ('e0000000-0000-0000-0000-000000000001','1785478598'),
  ('e0000000-0000-0000-0000-000000000002','1785478598'),
  ('e0000000-0000-0000-0000-000000000003','1785478598'),
  ('e0000000-0000-0000-0000-000000000004','1785478598'),
  ('e0000000-0000-0000-0000-000000000005','1785478598'),
  ('e0000000-0000-0000-0000-000000000006','1785478598'),
  ('7a000000-0000-0000-0000-00000000000a','1785478598'),
  ('7b000000-0000-0000-0000-00000000000b','1785478598')
) AS v(id, created_at);

-- e5 deliberately gets no name row at all.
INSERT INTO values (id, entity_id, property_id, space_id, text)
SELECT gen_random_uuid(), e.id, 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid,
       'bbbbbbbb-0000-0000-0000-000000000001'::uuid, 'fixture'
FROM entities e WHERE e.id NOT IN ('e0000000-0000-0000-0000-000000000005',
                                   'e0000000-0000-0000-0000-000000000006');

-- e6 HAS a name row, and it is blank. Load-bearing: with only the missing-row fixture,
-- deleting the `text IS NOT NULL AND length(trim(text)) > 0` check from the function still
-- passed every assertion, because the EXISTS failed on the absent row anyway. Mutation
-- testing caught that; this fixture is what makes the emptiness check observable.
INSERT INTO values (id, entity_id, property_id, space_id, text) VALUES
  (gen_random_uuid(), 'e0000000-0000-0000-0000-000000000006',
   'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid,
   'bbbbbbbb-0000-0000-0000-000000000001'::uuid, '   ');

INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system) VALUES
  ('0c000001-0000-0000-0000-000000000001','ea000000-0000-0000-0000-000000000001','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e0000000-0000-0000-0000-000000000001','7a000000-0000-0000-0000-00000000000a','bbbbbbbb-0000-0000-0000-000000000001',false),
  ('0c000002-0000-0000-0000-000000000002','ea000000-0000-0000-0000-000000000002','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e0000000-0000-0000-0000-000000000001','7b000000-0000-0000-0000-00000000000b','bbbbbbbb-0000-0000-0000-000000000001',false),
  ('0c000003-0000-0000-0000-000000000003','ea000000-0000-0000-0000-000000000003','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e0000000-0000-0000-0000-000000000002','7a000000-0000-0000-0000-00000000000a','bbbbbbbb-0000-0000-0000-000000000001',false),
  ('0c000004-0000-0000-0000-000000000004','ea000000-0000-0000-0000-000000000004','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e0000000-0000-0000-0000-000000000003','7b000000-0000-0000-0000-00000000000b','bbbbbbbb-0000-0000-0000-000000000001',false),
  ('0c000005-0000-0000-0000-000000000005','ea000000-0000-0000-0000-000000000005','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e0000000-0000-0000-0000-000000000004','7a000000-0000-0000-0000-00000000000a','bbbbbbbb-0000-0000-0000-000000000001',false),
  ('0c000006-0000-0000-0000-000000000006','ea000000-0000-0000-0000-000000000006','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e0000000-0000-0000-0000-000000000005','7a000000-0000-0000-0000-00000000000a','bbbbbbbb-0000-0000-0000-000000000001',false)

  ,('0c000007-0000-0000-0000-000000000007','ea000000-0000-0000-0000-000000000007','8f151ba4-de20-4e3c-9cb4-99ddf96f48f1','e0000000-0000-0000-0000-000000000006','7a000000-0000-0000-0000-00000000000a','bbbbbbbb-0000-0000-0000-000000000001',false);

INSERT INTO entity_feed_blocklist (entity_id, reason) VALUES
  ('e0000000-0000-0000-0000-000000000004','blocked fixture');

-- Push e2 and e3 above e1 — see the fixture header for why the cap assertion needs it.
INSERT INTO votes_count (object_id, object_type, space_id, vote_kind, positive, negative) VALUES
  ('e0000000-0000-0000-0000-000000000002', 0, 'bbbbbbbb-0000-0000-0000-000000000001', 0, 40, 0),
  ('e0000000-0000-0000-0000-000000000003', 0, 'bbbbbbbb-0000-0000-0000-000000000001', 0, 40, 0);

SELECT public.refresh_entity_ranking_scores(ARRAY(SELECT id FROM entities));

-- ---- risk 4: the signature is unambiguous ---------------------------------
-- If the DROP were missing, both the 5-arg and 6-arg versions would exist and this call
-- would raise "function ... is not unique" rather than returning rows. That is EVERY typed
-- feed request in production, so it is asserted before anything else.
SELECT assert((SELECT count(*) FROM pg_proc WHERE proname = 'entities_ranked_for_feed_by_type') = 1,
  'exactly one entities_ranked_for_feed_by_type exists — the old signature was dropped');

SELECT assert((SELECT count(*) FROM public.entities_ranked_for_feed_by_type(
                 ARRAY['7a000000-0000-0000-0000-00000000000a']::uuid[])) = 2,
  'a single-type call resolves and returns its renderable, unblocked members');

-- ---- risk 1: an entity with two requested types appears ONCE --------------
SELECT assert((SELECT count(*) FROM public.entities_ranked_for_feed_by_type(
                 ARRAY['7a000000-0000-0000-0000-00000000000a',
                       '7b000000-0000-0000-0000-00000000000b']::uuid[])
               WHERE id = 'e0000000-0000-0000-0000-000000000001') = 1,
  'a two-typed entity is returned once, not once per matching type (DISTINCT ON)');

SELECT assert((SELECT count(*) FROM public.entities_ranked_for_feed_by_type(
                 ARRAY['7a000000-0000-0000-0000-00000000000a',
                       '7b000000-0000-0000-0000-00000000000b']::uuid[])) = 3,
  'and the multi-type result is the union of members, de-duplicated (e1, e2, e3)');

-- ---- risk 3: the feed predicates still apply, from inside the lateral -----
SELECT assert((SELECT count(*) FROM public.entities_ranked_for_feed_by_type(
                 ARRAY['7a000000-0000-0000-0000-00000000000a']::uuid[])
               WHERE id = 'e0000000-0000-0000-0000-000000000004') = 0,
  'a blocklisted entity is never served, whatever it scores');

SELECT assert((SELECT count(*) FROM public.entities_ranked_for_feed_by_type(
                 ARRAY['7a000000-0000-0000-0000-00000000000a']::uuid[])
               WHERE id = 'e0000000-0000-0000-0000-000000000005') = 0,
  'an entity with no name row is not a candidate (0075)');

SELECT assert((SELECT count(*) FROM public.entities_ranked_for_feed_by_type(
                 ARRAY['7a000000-0000-0000-0000-00000000000a']::uuid[])
               WHERE id = 'e0000000-0000-0000-0000-000000000006') = 0,
  'nor is one whose name row is blank — emptiness is checked, not just presence');

-- The space filter reads the NAME's space, so a space nothing is named in yields nothing.
SELECT assert((SELECT count(*) FROM public.entities_ranked_for_feed_by_type(
                 ARRAY['7a000000-0000-0000-0000-00000000000a']::uuid[],
                 space_ids => ARRAY['cccccccc-0000-0000-0000-00000000000c']::uuid[])) = 0,
  'space_ids still narrows by where the entity is named');

SELECT assert((SELECT count(*) FROM public.entities_ranked_for_feed_by_type(
                 ARRAY['7a000000-0000-0000-0000-00000000000a']::uuid[],
                 space_ids => ARRAY['bbbbbbbb-0000-0000-0000-000000000001']::uuid[])) = 2,
  'and passing the right space returns them again — the previous assertion is not vacuous');

-- ---- risk 2: the cap ------------------------------------------------------
-- Capping at 1 per type must not silently drop the SECOND type's contribution: each type
-- still offers its own top row, so two distinct entities survive.
SELECT assert((SELECT count(*) FROM public.entities_ranked_for_feed_by_type(
                 ARRAY['7a000000-0000-0000-0000-00000000000a',
                       '7b000000-0000-0000-0000-00000000000b']::uuid[],
                 max_per_type => 1)) = 2,
  'max_per_type is PER TYPE, not global: each type still offers its own best row');

SELECT assert((SELECT count(*) FROM public.entities_ranked_for_feed_by_type(
                 ARRAY['7a000000-0000-0000-0000-00000000000a',
                       '7b000000-0000-0000-0000-00000000000b']::uuid[],
                 max_per_type => 66)) = 3,
  'a cap at or above the candidate count returns everything — same as uncapped');

-- NULL is Postgres's "no limit", not "limit zero". Getting this backwards would empty the
-- feed for every caller that does not pass a cap, which is all of them today.
SELECT assert((SELECT count(*) FROM public.entities_ranked_for_feed_by_type(
                 ARRAY['7a000000-0000-0000-0000-00000000000a',
                       '7b000000-0000-0000-0000-00000000000b']::uuid[],
                 max_per_type => NULL)) = 3,
  'an explicit NULL cap means unbounded, not empty');

-- ---- ordering is unchanged ------------------------------------------------
SELECT assert((SELECT array_agg(id ORDER BY ord) FROM (
                 SELECT id, row_number() OVER () AS ord
                 FROM public.entities_ranked_for_feed_by_type(
                   ARRAY['7a000000-0000-0000-0000-00000000000a',
                         '7b000000-0000-0000-0000-00000000000b']::uuid[])) q)
            = (SELECT array_agg(s.entity_id ORDER BY s.ranking_score DESC, s.entity_id DESC)
               FROM entity_ranking_scores s
               WHERE s.entity_id IN ('e0000000-0000-0000-0000-000000000001',
                                     'e0000000-0000-0000-0000-000000000002',
                                     'e0000000-0000-0000-0000-000000000003')),
  'rows come back in ranking_score DESC, entity_id DESC — the cursor order is preserved');

DROP FUNCTION assert(boolean, text);
