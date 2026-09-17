-- Assertions for 0092: the sampler measures the feed, not something adjacent to it.
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
-- 0091's sampler passed its own tests while being wrong in two ways that its tests could
-- not see, because those tests built fixtures that were all valid feed candidates in one
-- space with one type each. The assertions here are the ones that would have caught it:
--
--   1. space scoping actually filters (0091 recorded space_id and ignored it)
--   2. an entity with two watched types takes ONE slot (0091 counted pairs)
--   3. a non-candidate — unnamed, system, blocklisted — never appears in a sample
--
-- Each of those fails against 0091's implementation and passes against this one.

\set ON_ERROR_STOP on

-- `IS NOT TRUE`, not `NOT cond`: a NULL condition would otherwise take the ELSE branch and
-- report a pass. See README.md.
CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

DELETE FROM entity_feed_composition_samples WHERE type_ids @> ARRAY['92000000-0000-4000-8000-0000000000aa'::uuid];
DELETE FROM entity_type_ranking   WHERE entity_id::text LIKE '00000092-%';
DELETE FROM entity_ranking_scores WHERE entity_id::text LIKE '00000092-%';
DELETE FROM entity_feed_blocklist WHERE entity_id::text LIKE '00000092-%';
DELETE FROM relations             WHERE from_entity_id::text LIKE '00000092-%';
DELETE FROM values                WHERE entity_id::text LIKE '00000092-%';
DELETE FROM entities              WHERE id::text        LIKE '00000092-%';

UPDATE entity_ranking_config
   SET participation_weight = 4.0, participation_cap = 19,
       comment_weight = 0, comment_cap = 30, tau_seconds = 100000
 WHERE id;

-- ---------------------------------------------------------------------------
-- Fixtures.
--   ...01  type A,        named in SPACE_1
--   ...02  type A,        named in SPACE_2
--   ...03  types A AND B, named in SPACE_1   -- the double-count case
--   ...04  type A,        NO name            -- not a feed candidate (0075)
--   ...05  type A,        named in SPACE_1, blocklisted
-- ---------------------------------------------------------------------------
INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
SELECT v.id::uuid, v.created_at::text, '0', v.created_at::text, '0' FROM (VALUES
  ('00000092-0000-4000-8000-000000000001','1786000000'),
  ('00000092-0000-4000-8000-000000000002','1786000000'),
  ('00000092-0000-4000-8000-000000000003','1786000000'),
  ('00000092-0000-4000-8000-000000000004','1786000000'),
  ('00000092-0000-4000-8000-000000000005','1786000000')
) AS v(id, created_at);

INSERT INTO values (id, entity_id, property_id, space_id, text) VALUES
  (gen_random_uuid()::text,'00000092-0000-4000-8000-000000000001','a126ca53-0c8e-48d5-b888-82c734c38935','92000000-0000-4000-8000-000000000f01','in space 1'),
  (gen_random_uuid()::text,'00000092-0000-4000-8000-000000000002','a126ca53-0c8e-48d5-b888-82c734c38935','92000000-0000-4000-8000-000000000f02','in space 2'),
  (gen_random_uuid()::text,'00000092-0000-4000-8000-000000000003','a126ca53-0c8e-48d5-b888-82c734c38935','92000000-0000-4000-8000-000000000f01','two types'),
  (gen_random_uuid()::text,'00000092-0000-4000-8000-000000000005','a126ca53-0c8e-48d5-b888-82c734c38935','92000000-0000-4000-8000-000000000f01','blocklisted');
-- ...04 deliberately has no name row.

INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
SELECT gen_random_uuid(), gen_random_uuid(), '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid,
       v.ent::uuid, v.typ::uuid, '92000000-0000-4000-8000-000000000f01'::uuid, false
  FROM (VALUES
  ('00000092-0000-4000-8000-000000000001','92000000-0000-4000-8000-0000000000aa'),
  ('00000092-0000-4000-8000-000000000002','92000000-0000-4000-8000-0000000000aa'),
  ('00000092-0000-4000-8000-000000000003','92000000-0000-4000-8000-0000000000aa'),
  ('00000092-0000-4000-8000-000000000003','92000000-0000-4000-8000-0000000000bb'),
  ('00000092-0000-4000-8000-000000000004','92000000-0000-4000-8000-0000000000aa'),
  ('00000092-0000-4000-8000-000000000005','92000000-0000-4000-8000-0000000000aa')
) AS v(ent, typ);

INSERT INTO entity_feed_blocklist (entity_id) VALUES ('00000092-0000-4000-8000-000000000005');

SELECT assert(public.refresh_entity_ranking_scores(ARRAY(
  SELECT id FROM entities WHERE id::text LIKE '00000092-%')) = 5, 'fixtures scored');

-- ---------------------------------------------------------------------------
-- 1. Space scoping. 0091 recorded space_id and sampled globally regardless, so this
--    assertion is the one that pins the behaviour the column always claimed.
-- ---------------------------------------------------------------------------
DO $$
DECLARE s1 bigint; s2 bigint; n1 int; n2 int;
BEGIN
  s1 := public.sample_feed_composition(
          ARRAY['92000000-0000-4000-8000-0000000000aa'::uuid], 66,
          '92000000-0000-4000-8000-000000000f01');
  s2 := public.sample_feed_composition(
          ARRAY['92000000-0000-4000-8000-0000000000aa'::uuid], 66,
          '92000000-0000-4000-8000-000000000f02');
  SELECT COALESCE(sum(value::int), 0) INTO n1
    FROM entity_feed_composition_samples s, jsonb_each_text(s.composition) WHERE s.id = s1;
  SELECT COALESCE(sum(value::int), 0) INTO n2
    FROM entity_feed_composition_samples s, jsonb_each_text(s.composition) WHERE s.id = s2;
  -- space 1 holds ...01 and ...03 (…04 unnamed, …05 blocklisted); space 2 holds ...02.
  PERFORM assert(n1 = 2, format('space 1 sees its own 2 entities, not everything (got %s)', n1));
  PERFORM assert(n2 = 1, format('space 2 sees only its own 1 entity (got %s)', n2));
END $$;

-- ---------------------------------------------------------------------------
-- 2. An entity with two watched types takes ONE slot. 0091 counted (type, entity)
--    pairs, so this returned 3 where the feed shows 2.
-- ---------------------------------------------------------------------------
DO $$
DECLARE sid bigint; total int; a int;
BEGIN
  sid := public.sample_feed_composition(
           ARRAY['92000000-0000-4000-8000-0000000000aa'::uuid,
                 '92000000-0000-4000-8000-0000000000bb'::uuid], 66,
           '92000000-0000-4000-8000-000000000f01');
  SELECT COALESCE(sum(value::int), 0) INTO total
    FROM entity_feed_composition_samples s, jsonb_each_text(s.composition) WHERE s.id = sid;
  SELECT COALESCE((composition ->> '92000000-0000-4000-8000-0000000000aa')::int, 0) INTO a
    FROM entity_feed_composition_samples WHERE id = sid;
  PERFORM assert(total = 2,
    format('an entity with two watched types occupies one slot, not two (window held %s)', total));
  PERFORM assert(a = 2,
    format('it is labelled with the caller''s first matching type (type A count %s)', a));
END $$;

-- ---------------------------------------------------------------------------
-- 3. Non-candidates never appear. 0091 read entity_type_ranking directly and applied
--    none of the feed's predicates, so an unnamed or blocklisted entity could hold a
--    slot in the sample that it could never hold in the feed.
-- ---------------------------------------------------------------------------
DO $$
DECLARE sid bigint; total int;
BEGIN
  sid := public.sample_feed_composition(
           ARRAY['92000000-0000-4000-8000-0000000000aa'::uuid], 66, NULL);
  SELECT COALESCE(sum(value::int), 0) INTO total
    FROM entity_feed_composition_samples s, jsonb_each_text(s.composition) WHERE s.id = sid;
  -- Unscoped: ...01, ...02, ...03 qualify. ...04 has no name, ...05 is blocklisted.
  PERFORM assert(total = 3,
    format('the unnamed and the blocklisted entity are absent from the sample (got %s of 3)', total));
END $$;

-- ---------------------------------------------------------------------------
-- 4. The window still truncates, and the config snapshot still travels with it.
-- ---------------------------------------------------------------------------
DO $$
DECLARE sid bigint; total int; w numeric;
BEGIN
  sid := public.sample_feed_composition(
           ARRAY['92000000-0000-4000-8000-0000000000aa'::uuid], 1, NULL);
  SELECT COALESCE(sum(value::int), 0) INTO total
    FROM entity_feed_composition_samples s, jsonb_each_text(s.composition) WHERE s.id = sid;
  PERFORM assert(total = 1, format('a window of 1 counts 1 row (got %s)', total));

  UPDATE entity_ranking_config SET participation_weight = 5.5 WHERE id;
  sid := public.sample_feed_composition(
           ARRAY['92000000-0000-4000-8000-0000000000aa'::uuid], 66, NULL);
  SELECT participation_weight INTO w FROM entity_feed_composition_samples WHERE id = sid;
  PERFORM assert(w = 5.5, format('the config snapshot follows the config (%s)', w));
  UPDATE entity_ranking_config SET participation_weight = 4.0 WHERE id;
END $$;

-- ---------------------------------------------------------------------------
-- 5. Drift is computed per scope. A space that moved must not be reported against
--    the global history, or every per-space sample would look like a global change.
-- ---------------------------------------------------------------------------
DO $$
DECLARE n_global int; n_space int;
BEGIN
  DELETE FROM entity_feed_composition_samples
   WHERE type_ids @> ARRAY['92000000-0000-4000-8000-0000000000aa'::uuid];
  PERFORM public.sample_feed_composition(ARRAY['92000000-0000-4000-8000-0000000000aa'::uuid], 66, NULL);
  PERFORM public.sample_feed_composition(ARRAY['92000000-0000-4000-8000-0000000000aa'::uuid], 66,
                                         '92000000-0000-4000-8000-000000000f01');
  SELECT count(*) INTO n_global FROM public.feed_composition_drift(NULL);
  SELECT count(*) INTO n_space  FROM public.feed_composition_drift('92000000-0000-4000-8000-000000000f01');
  PERFORM assert(n_global = 0,
    format('one global sample plus one space sample is not global drift (got %s rows)', n_global));
  PERFORM assert(n_space = 0,
    format('the space has only one sample of its own, so no drift (got %s rows)', n_space));
END $$;

DELETE FROM entity_feed_composition_samples WHERE type_ids @> ARRAY['92000000-0000-4000-8000-0000000000aa'::uuid];
DELETE FROM entity_type_ranking   WHERE entity_id::text LIKE '00000092-%';
DELETE FROM entity_ranking_scores WHERE entity_id::text LIKE '00000092-%';
DELETE FROM entity_feed_blocklist WHERE entity_id::text LIKE '00000092-%';
DELETE FROM relations             WHERE from_entity_id::text LIKE '00000092-%';
DELETE FROM values                WHERE entity_id::text LIKE '00000092-%';
DELETE FROM entities              WHERE id::text        LIKE '00000092-%';

SELECT 'ALL 0092 ASSERTIONS PASSED' AS result;
