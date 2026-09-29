-- Assertions for 0095: onboarding topic suggestions (GEO-3079).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
-- Two spaces:
--   A ("AI" space, own topic T_OWN "AI"): Compute (3 claims), Safety (2), Shared (1),
--     a second entity also named "ai" (4, must be dropped as the space's own topic by name),
--     the own topic itself (5, dropped by id), a non-Topic "Useful Links" (6, dropped),
--     an unnamed Topic (7, dropped), and a topic linked only from a non-claim (dropped).
--   B (no own topic): Mental health twice under two ids (3 and 1 links, one kept),
--     Shared (2), Sleep (1).

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

-- Ids. 95…-00a* are spaces, 95…-01* topics, 95…-02* content.
CREATE TEMP TABLE ids AS SELECT * FROM (VALUES
  ('space_a',   '95000000-0000-4000-8000-0000000000a1'::uuid),
  ('space_b',   '95000000-0000-4000-8000-0000000000a2'::uuid),
  ('t_own',     '95000000-0000-4000-8000-000000000101'::uuid),
  ('t_ai_dup',  '95000000-0000-4000-8000-000000000102'::uuid),
  ('t_compute', '95000000-0000-4000-8000-000000000103'::uuid),
  ('t_safety',  '95000000-0000-4000-8000-000000000104'::uuid),
  ('t_shared',  '95000000-0000-4000-8000-000000000105'::uuid),
  ('t_links',   '95000000-0000-4000-8000-000000000106'::uuid),
  ('t_unnamed', '95000000-0000-4000-8000-000000000107'::uuid),
  ('t_noteonly','95000000-0000-4000-8000-000000000108'::uuid),
  ('t_mental1', '95000000-0000-4000-8000-000000000109'::uuid),
  ('t_mental2', '95000000-0000-4000-8000-00000000010a'::uuid),
  ('t_sleep',   '95000000-0000-4000-8000-00000000010b'::uuid)
) AS v(key, id);

CREATE OR REPLACE FUNCTION pg_temp.id(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM ids WHERE key = k $$;

-- Every entity referenced gets an entities row (topics are returned as entities).
INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
SELECT id, '1790000000', '0', '1790000000', '0' FROM ids WHERE key LIKE 't_%';

INSERT INTO spaces (id, type, address, topic_id) VALUES
  (pg_temp.id('space_a'), 'DAO', '0x95a', pg_temp.id('t_own')),
  (pg_temp.id('space_b'), 'DAO', '0x95b', NULL);

CREATE OR REPLACE FUNCTION pg_temp.rel(from_id uuid, type_id uuid, to_id uuid, space uuid) RETURNS void
LANGUAGE sql AS $$
  INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id)
  VALUES (gen_random_uuid(), gen_random_uuid(), type_id, from_id, to_id, space)
$$;
CREATE OR REPLACE FUNCTION pg_temp.name(entity uuid, label text) RETURNS void
LANGUAGE sql AS $$
  INSERT INTO values (id, property_id, entity_id, space_id, text)
  VALUES (gen_random_uuid()::text, 'a126ca53-0c8e-48d5-b888-82c734c38935', entity,
          '95000000-0000-4000-8000-0000000000a1', label)
$$;

DO $$
DECLARE
  topics constant uuid := '806d52bc-27e9-4c91-93c0-57978b093351';
  types  constant uuid := '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1';
  claim  constant uuid := '96f859ef-a1ca-4b22-9372-c86ad58b694b';
  debate constant uuid := 'fd51f935-2063-4617-be39-7b672b23364c';
  topic  constant uuid := '5ef5a586-0f27-4d8e-8f6c-59ae5b3e89e2';
  note   constant uuid := '95000000-0000-4000-8000-0000000009ff';
  a uuid := pg_temp.id('space_a');
  b uuid := pg_temp.id('space_b');
  -- (topic key, space, how many claims link to it)
  spec record;
  n int := 0;
  item uuid;
BEGIN
  -- Topic types and names. t_links is not a Topic; t_unnamed has no name.
  PERFORM pg_temp.rel(id, types, topic, a) FROM ids WHERE key LIKE 't_%' AND key <> 't_links';
  PERFORM pg_temp.name(pg_temp.id('t_own'), 'AI');
  PERFORM pg_temp.name(pg_temp.id('t_ai_dup'), 'ai ');
  PERFORM pg_temp.name(pg_temp.id('t_compute'), 'Compute');
  PERFORM pg_temp.name(pg_temp.id('t_safety'), 'Safety');
  PERFORM pg_temp.name(pg_temp.id('t_shared'), 'U.S.–China relations');
  PERFORM pg_temp.name(pg_temp.id('t_links'), 'Useful Links');
  PERFORM pg_temp.name(pg_temp.id('t_noteonly'), 'Note topic');
  PERFORM pg_temp.name(pg_temp.id('t_mental1'), 'Mental health');
  PERFORM pg_temp.name(pg_temp.id('t_mental2'), 'mental  health');
  PERFORM pg_temp.name(pg_temp.id('t_mental2'), 'Mental health');
  PERFORM pg_temp.name(pg_temp.id('t_sleep'), 'Sleep');

  FOR spec IN SELECT * FROM (VALUES
    ('t_compute', a, 3), ('t_safety', a, 2), ('t_shared', a, 1), ('t_ai_dup', a, 4),
    ('t_own', a, 5), ('t_links', a, 6), ('t_unnamed', a, 7),
    ('t_mental1', b, 3), ('t_mental2', b, 1), ('t_shared', b, 2), ('t_sleep', b, 1),
    -- B's content links to the AI space's topic by name: fine alone, not once A is chosen too.
    ('t_ai_dup', b, 1)
  ) AS s(topic_key, space, claims) LOOP
    FOR i IN 1..spec.claims LOOP
      n := n + 1;
      item := ('95000000-0000-4000-8000-' || lpad(to_hex(512 + n), 12, '0'))::uuid;
      -- Alternate claims and debates: both count.
      PERFORM pg_temp.rel(item, types, CASE WHEN n % 2 = 0 THEN debate ELSE claim END, spec.space);
      PERFORM pg_temp.rel(item, topics, pg_temp.id(spec.topic_key), spec.space);
    END LOOP;
  END LOOP;

  -- A note (not a claim or debate) linking to its own topic: never counted.
  PERFORM pg_temp.rel(note, types, '95000000-0000-4000-8000-0000000009fe', a);
  PERFORM pg_temp.rel(note, topics, pg_temp.id('t_noteonly'), a);
END $$;

SELECT public.refresh_space_topic_suggestions();

-- ---------------------------------------------------------------------------
-- 1. Per space: only real topics, never the space's own, one per name.
-- ---------------------------------------------------------------------------
DO $$
DECLARE got text;
BEGIN
  SELECT string_agg(name_key || ':' || links, ', ' ORDER BY rank) INTO got
    FROM space_topic_suggestions WHERE space_id = pg_temp.id('space_a');
  PERFORM assert(got = 'compute:3, safety:2, u.s.–china relations:1',
    format('space A is Compute, Safety, Shared, with its own topic (by id and by name), the non-Topic, the unnamed and the note-only topic all gone (got %s)', got));

  SELECT string_agg(name_key || ':' || links, ', ' ORDER BY rank) INTO got
    FROM space_topic_suggestions WHERE space_id = pg_temp.id('space_b');
  PERFORM assert(got = 'mental health:3, u.s.–china relations:2, ai:1, sleep:1',
    format('space B keeps one Mental health, the most linked (got %s)', got));
END $$;

-- ---------------------------------------------------------------------------
-- 2. THE READ. Two spaces interleave by rank, and a shared topic appears once.
-- ---------------------------------------------------------------------------
DO $$
DECLARE got text;
BEGIN
  SELECT string_agg(lower(v.text), ', ' ORDER BY ord) INTO got
  FROM (SELECT t.id, t.ordinality AS ord
          FROM public.topic_suggestions_for_spaces(ARRAY[pg_temp.id('space_a'), pg_temp.id('space_b')])
               WITH ORDINALITY AS t) t
  JOIN LATERAL (SELECT text FROM values WHERE entity_id = t.id
                  AND property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935' ORDER BY text LIMIT 1) v ON true;
  -- Rank 1s first (Compute 3, Mental health 3), then rank 2s (Safety 2 and Shared 2, at its best
  -- rank, B's; equal links, so id order), then Sleep. Shared, rank 3 in A, is not repeated, and
  -- "ai" from B is gone because A, whose own topic is AI, was chosen too.
  PERFORM assert(got = 'compute, mental health, safety, u.s.–china relations, sleep',
    format('both spaces are represented at the top and the shared topic appears once (got %s)', got));
END $$;

DO $$
DECLARE got int;
BEGIN
  SELECT count(*) INTO got
  FROM public.topic_suggestions_for_spaces(ARRAY[pg_temp.id('space_b')]) t
  WHERE t.id = pg_temp.id('t_ai_dup');
  PERFORM assert(got = 1, format('B alone still offers "ai": the AI space was not chosen (got %s)', got));
END $$;

-- ---------------------------------------------------------------------------
-- 3. A refresh replaces the ranking rather than adding to it.
-- ---------------------------------------------------------------------------
DO $$
DECLARE before int; after int;
BEGIN
  SELECT count(*) INTO before FROM space_topic_suggestions WHERE space_id IN (pg_temp.id('space_a'), pg_temp.id('space_b'));
  PERFORM public.refresh_space_topic_suggestions();
  SELECT count(*) INTO after FROM space_topic_suggestions WHERE space_id IN (pg_temp.id('space_a'), pg_temp.id('space_b'));
  PERFORM assert(before = 7 AND after = 7, format('refreshing twice leaves 7 rows, not 14 (got %s then %s)', before, after));
END $$;
