-- Assertions for 0114: one API query for the current Debate agendas in a set of spaces (GEO-3503).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
--   1. Spaces come back in the order asked; personal spaces and spaces with no agenda return nothing.
--   2. A space's current agenda is its freshest arrived one; on the same Date, the later-created one.
--      A future-dated agenda is ignored.
--   3. Contents are block items only, sections and claims in position order (byte order, so a
--      mixed-case key sorts as it was generated); non-claims and items written in another space are
--      left out; each section carries its name and its first Topics target in the agenda's space.
--   4. The agenda's own Claims and Topics come back as separate rows after its contents.
--   5. Earlier listings: other agendas in the window, once per claim at its newest listing, none
--      that is on a current agenda; off by default.
--   6. lastListedOn: the claim's newest listing in any space, including a block item added since
--      the hourly sweep; null for topics.
--   7. 0113's listings are unchanged: read from the same contents, a claim in two blocks is one row.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

TRUNCATE entities, values, relations, spaces, entity_ranking_scores, entity_type_ranking,
         entity_topic_ranking, trending.agenda_listings CASCADE;

-- ---------------------------------------------------------------------------
-- Fixtures. Days are New York calendar days relative to today.
--   S1, S2 DAO spaces; SP a personal space; S3 a DAO space with no agenda.
--   ANEW, AOLD   S1, both dated today; ANEW created later, so it is current.
--     ANEW  blocks BY (pos a0, "Topic Y", Topics TOPY) then BX (pos a1, "Topic X", its Topics written
--           in S2). BY: C5, PAGE (not a claim), C1. BX: C4 (pos Zz), C3 (pos a0), C5, C12 (written
--           in S2). Own Claims → C6, own Topics → TOPA.
--     AOLD  C1, C2.
--   AY     S1, yesterday: C2, C3, C14.
--   A10    S1, 10 days ago: C8 (outside a 6-day window).
--   AFUT   S1, tomorrow: C9.
--   AS2    S2, 2 days ago: C7, C10.   AS2Y  S2, 3 days ago: C11.
--   AP     SP (personal), today: C15.
--   After the hourly sweep, C13 is added to BX (pos b0).
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE ids AS
SELECT k, ('b1140000-0000-4000-8000-' || lpad(to_hex(n), 12, '0'))::uuid AS id
FROM unnest(ARRAY['S1','S2','S3','SP','ANEW','AOLD','AY','A10','AFUT','AS2','AS2Y','AP','BY','BX',
                  'BOLD','BAY','BA10','BAFUT','BAS2','BAS2Y','BAP',
                  'C1','C2','C3','C4','C5','C6','C7','C8','C9','C10','C11','C12','C13','C14','C15',
                  'PAGE','TOPY','TOPA']) WITH ORDINALITY AS u(k, n)
UNION ALL SELECT * FROM (VALUES
  ('CLAIM',  '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid),
  ('AGENDA', '79e3757b-2630-492c-9d3b-d73a78d45893'::uuid),
  ('PAGET',  '480e3fc2-67f3-4993-85fb-cdf4ddeaa6b0'::uuid)) AS v(k, id);
CREATE OR REPLACE FUNCTION pg_temp.i(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM ids WHERE ids.k = $1 $$;
CREATE OR REPLACE FUNCTION pg_temp.today() RETURNS date LANGUAGE sql AS $$
  SELECT (now() AT TIME ZONE 'America/New_York')::date $$;

CREATE OR REPLACE FUNCTION pg_temp.ent(k text, created text) RETURNS void LANGUAGE sql AS $$
  INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
  VALUES (pg_temp.i(k), created, '0', created, '0') ON CONFLICT DO NOTHING $$;
CREATE OR REPLACE FUNCTION pg_temp.rel(from_k text, type_id uuid, to_k text, space_k text, pos text DEFAULT NULL)
RETURNS void LANGUAGE sql AS $$
  INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system, position)
  VALUES (gen_random_uuid(), gen_random_uuid(), type_id, pg_temp.i(from_k), pg_temp.i(to_k), pg_temp.i(space_k), false, pos)
$$;
CREATE OR REPLACE FUNCTION pg_temp.typed(k text, type_k text, space_k text) RETURNS void LANGUAGE sql AS $$
  SELECT pg_temp.rel(k, '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid, type_k, space_k) $$;
CREATE OR REPLACE FUNCTION pg_temp.name(k text, space_k text, name text) RETURNS void LANGUAGE sql AS $$
  INSERT INTO values (id, entity_id, property_id, space_id, text)
  VALUES (gen_random_uuid()::text, pg_temp.i(k), 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid, pg_temp.i(space_k), name) $$;
-- A Debate agenda in `space_k` dated `d` days ago (negative: the future), created at `created`.
CREATE OR REPLACE FUNCTION pg_temp.agenda(k text, space_k text, d integer, created text) RETURNS void
LANGUAGE sql AS $$
  SELECT pg_temp.ent(k, created);
  SELECT pg_temp.typed(k, 'AGENDA', space_k);
  INSERT INTO values (id, entity_id, property_id, space_id, datetime)
  VALUES (gen_random_uuid()::text, pg_temp.i(k), '52665f3e-fb7d-48d5-8b6b-abb21b0d36db'::uuid, pg_temp.i(space_k),
          (pg_temp.today() - d)::text || 'T00:00:00-04:00');
$$;
-- A block on agenda `agenda_k` at `pos`, with `claims` as its items in that order (positions a0, a1, …).
CREATE OR REPLACE FUNCTION pg_temp.block(agenda_k text, block_k text, space_k text, pos text, claims text[]) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE n integer := 0; c text;
BEGIN
  PERFORM pg_temp.rel(agenda_k, 'beaba5cb-a677-41a8-b353-77030613fc70'::uuid, block_k, space_k, pos);
  FOREACH c IN ARRAY claims LOOP
    PERFORM pg_temp.rel(block_k, 'a99f9ce1-2ffa-4dac-8c61-f6310d46064a'::uuid, c, space_k, 'a' || n);
    n := n + 1;
  END LOOP;
END $$;
CREATE OR REPLACE FUNCTION pg_temp.item(block_k text, claim_k text, space_k text, pos text) RETURNS void LANGUAGE sql AS $$
  SELECT pg_temp.rel(block_k, 'a99f9ce1-2ffa-4dac-8c61-f6310d46064a'::uuid, claim_k, space_k, pos) $$;

INSERT INTO spaces (id, type, address)
SELECT id, CASE WHEN k = 'SP' THEN 'Personal' ELSE 'DAO' END::"spaceTypes", 'addr-' || k
FROM ids WHERE k IN ('S1','S2','S3','SP');

SELECT pg_temp.ent(k, '1780000000'), pg_temp.typed(k, 'CLAIM', 'S1') FROM ids
WHERE k IN ('C1','C2','C3','C4','C5','C6','C7','C8','C9','C10','C11','C12','C13','C14','C15');
SELECT pg_temp.ent(k, '1780000000') FROM ids
WHERE k IN ('PAGE','TOPY','TOPA','BY','BX','BOLD','BAY','BA10','BAFUT','BAS2','BAS2Y','BAP');
SELECT pg_temp.typed('PAGE', 'PAGET', 'S1');

SELECT pg_temp.agenda('AOLD', 'S1', 0, '1790000000');
SELECT pg_temp.block('AOLD', 'BOLD', 'S1', 'a0', ARRAY['C1','C2']);

SELECT pg_temp.agenda('ANEW', 'S1', 0, '1790000500');
SELECT pg_temp.rel('ANEW', 'beaba5cb-a677-41a8-b353-77030613fc70'::uuid, 'BX', 'S1', 'a1');
SELECT pg_temp.block('ANEW', 'BY', 'S1', 'a0', ARRAY['C5','PAGE','C1']);
SELECT pg_temp.item('BX', 'C3', 'S1', 'a0'), pg_temp.item('BX', 'C4', 'S1', 'Zz'),
       pg_temp.item('BX', 'C5', 'S1', 'a5'), pg_temp.item('BX', 'C12', 'S2', 'a6');
SELECT pg_temp.name('BY', 'S1', 'Topic Y'), pg_temp.name('BX', 'S1', 'Topic X');
SELECT pg_temp.rel('BY', '806d52bc-27e9-4c91-93c0-57978b093351'::uuid, 'TOPY', 'S1');
SELECT pg_temp.rel('BX', '806d52bc-27e9-4c91-93c0-57978b093351'::uuid, 'TOPY', 'S2');
SELECT pg_temp.rel('ANEW', 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid, 'C6', 'S1');
SELECT pg_temp.rel('ANEW', '806d52bc-27e9-4c91-93c0-57978b093351'::uuid, 'TOPA', 'S1');

SELECT pg_temp.agenda('AY', 'S1', 1, '1790000000'),   pg_temp.block('AY', 'BAY', 'S1', 'a0', ARRAY['C2','C3','C14']);
SELECT pg_temp.agenda('A10', 'S1', 10, '1790000000'), pg_temp.block('A10', 'BA10', 'S1', 'a0', ARRAY['C8']);
SELECT pg_temp.agenda('AFUT', 'S1', -1, '1790000000'), pg_temp.block('AFUT', 'BAFUT', 'S1', 'a0', ARRAY['C9']);
SELECT pg_temp.agenda('AS2', 'S2', 2, '1790000000'),  pg_temp.block('AS2', 'BAS2', 'S2', 'a0', ARRAY['C7','C10']);
SELECT pg_temp.agenda('AS2Y', 'S2', 3, '1790000000'), pg_temp.block('AS2Y', 'BAS2Y', 'S2', 'a0', ARRAY['C11']);
SELECT pg_temp.agenda('AP', 'SP', 0, '1790000000'),   pg_temp.block('AP', 'BAP', 'SP', 'a0', ARRAY['C15']);

-- The hourly sweep fills the listings table; then a block item arrives that it has not seen.
SELECT trending.refresh_agenda_listings(true);
SELECT pg_temp.item('BX', 'C13', 'S1', 'b0');

CREATE TEMP TABLE got AS
SELECT row_number() OVER () AS n, r.*
FROM public.current_debate_agendas(ARRAY[pg_temp.i('S2'), pg_temp.i('S1'), pg_temp.i('SP'), pg_temp.i('S3')], 6) r;
CREATE OR REPLACE FUNCTION pg_temp.k(id uuid) RETURNS text LANGUAGE sql AS $$ SELECT k FROM ids WHERE ids.id = $1 $$;
-- The keys of the rows matching a filter, in returned order.
CREATE OR REPLACE FUNCTION pg_temp.seq(q text) RETURNS text[] LANGUAGE plpgsql AS $$
DECLARE out text[];
BEGIN
  EXECUTE 'SELECT array_agg(pg_temp.k(entity_id) ORDER BY n) FROM got WHERE ' || q INTO out;
  RETURN out;
END $$;

-- ---------------------------------------------------------------------------
-- 1. Spaces.
-- ---------------------------------------------------------------------------
SELECT assert((SELECT array_agg(DISTINCT pg_temp.k(space_id)) FROM got) = ARRAY['S1','S2'],
              '1: only spaces with an arrived agenda in a DAO space return rows (not SP, not S3)');
SELECT assert((SELECT max(n) FROM got WHERE space_id = pg_temp.i('S2')) < (SELECT min(n) FROM got WHERE space_id = pg_temp.i('S1')),
              '1: spaces come back in the order asked');
SELECT assert(NOT EXISTS (SELECT 1 FROM public.current_debate_agendas('{}'::uuid[], 6)),
              '1: no spaces, no rows');

-- ---------------------------------------------------------------------------
-- 2. The current agenda.
-- ---------------------------------------------------------------------------
SELECT assert((SELECT array_agg(DISTINCT pg_temp.k(agenda_id)) FROM got WHERE is_current AND space_id = pg_temp.i('S1')) = ARRAY['ANEW'],
              '2: of two agendas dated today, the later-created one is current');
SELECT assert((SELECT array_agg(DISTINCT pg_temp.k(agenda_id)) FROM got WHERE is_current AND space_id = pg_temp.i('S2')) = ARRAY['AS2']
              AND (SELECT DISTINCT listed_on FROM got WHERE is_current AND space_id = pg_temp.i('S2')) = pg_temp.today() - 2,
              '2: a space''s freshest arrived agenda is current even when it is not today''s');
SELECT assert(NOT EXISTS (SELECT 1 FROM got WHERE agenda_id = pg_temp.i('AFUT') OR entity_id = pg_temp.i('C9')),
              '2: a future-dated agenda is ignored');

-- ---------------------------------------------------------------------------
-- 3. Contents.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.seq('is_current AND source = ''block_item'' AND space_id = pg_temp.i(''S1'')')
              = ARRAY['C5','C1','C4','C3','C5','C13'],
              '3: block items, sections then claims in position order (Zz before a0: byte order)');
SELECT assert(NOT EXISTS (SELECT 1 FROM got WHERE entity_id IN (pg_temp.i('PAGE'), pg_temp.i('C12'))),
              '3: a non-claim and an item written in another space are left out');
SELECT assert((SELECT bool_and(section_id = pg_temp.i('BY') AND section_name = 'Topic Y' AND section_topic_id = pg_temp.i('TOPY')
                               AND section_position = 1)
               FROM got WHERE is_current AND source = 'block_item' AND n < (SELECT min(n) FROM got WHERE section_id = pg_temp.i('BX'))
                 AND space_id = pg_temp.i('S1')),
              '3: the first section carries its block, name and topic');
SELECT assert((SELECT bool_and(section_name = 'Topic X' AND section_topic_id IS NULL AND section_position = 2)
               FROM got WHERE section_id = pg_temp.i('BX')),
              '3: a section''s Topics written in another space is not its topic');

-- ---------------------------------------------------------------------------
-- 4. Ranking levers.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.seq('source <> ''block_item''') = ARRAY['C6','TOPA'],
              '4: the agenda''s own Claims then Topics, as their own sources');
SELECT assert((SELECT bool_and(section_id IS NULL) FROM got WHERE source <> 'block_item')
              AND (SELECT min(n) FROM got WHERE source <> 'block_item' AND space_id = pg_temp.i('S1'))
                > (SELECT max(n) FROM got WHERE is_current AND source = 'block_item' AND space_id = pg_temp.i('S1')),
              '4: after the agenda''s contents, with no section');

-- ---------------------------------------------------------------------------
-- 5. Earlier listings.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.seq('NOT is_current AND space_id = pg_temp.i(''S1'')') = ARRAY['C2','C14'],
              '5: S1''s other agendas in the window, newest first (AOLD today, then AY)');
SELECT assert(pg_temp.seq('NOT is_current AND space_id = pg_temp.i(''S2'')') = ARRAY['C11'],
              '5: per space');
SELECT assert((SELECT count(*) FROM got WHERE entity_id = pg_temp.i('C2')) = 1
              AND (SELECT listed_on FROM got WHERE entity_id = pg_temp.i('C2')) = pg_temp.today(),
              '5: once per claim, at its newest listing');
SELECT assert(NOT EXISTS (SELECT 1 FROM got WHERE NOT is_current AND entity_id IN (pg_temp.i('C1'), pg_temp.i('C3'), pg_temp.i('C8'))),
              '5: none that is on a current agenda, and none older than the window');
SELECT assert(NOT EXISTS (SELECT 1 FROM public.current_debate_agendas(ARRAY[pg_temp.i('S1')]) r WHERE NOT r.is_current),
              '5: off by default');

-- ---------------------------------------------------------------------------
-- 6. lastListedOn.
-- ---------------------------------------------------------------------------
SELECT assert((SELECT last_listed_on FROM got WHERE entity_id = pg_temp.i('C13')) = pg_temp.today(),
              '6: a block item the hourly sweep has not seen yet is listed today');
SELECT assert((SELECT last_listed_on FROM got WHERE entity_id = pg_temp.i('C14')) = pg_temp.today() - 1
              AND (SELECT last_listed_on FROM got WHERE entity_id = pg_temp.i('C7')) = pg_temp.today() - 2,
              '6: otherwise the claim''s newest listing');
SELECT assert((SELECT last_listed_on FROM got WHERE entity_id = pg_temp.i('TOPA')) IS NULL
              AND (SELECT last_listed_on FROM got WHERE entity_id = pg_temp.i('C6')) = pg_temp.today(),
              '6: null for a topic; a Claims target is a listing');

-- ---------------------------------------------------------------------------
-- 7. 0113's listings.
-- ---------------------------------------------------------------------------
SELECT assert((SELECT count(*) FROM trending.current_agenda_listings()
               WHERE agenda_id = pg_temp.i('ANEW') AND entity_id = pg_temp.i('C5')) = 1,
              '7: a claim in two blocks is one listing');
SELECT assert((SELECT array_agg(source ORDER BY source) FROM trending.current_agenda_listings() WHERE agenda_id = pg_temp.i('ANEW'))
              = ARRAY['agenda_claims','agenda_topics','block_item','block_item','block_item','block_item','block_item'],
              '7: block items (C1 C3 C4 C5 C13 and not PAGE or C12), its Claims and its Topics');

-- Cleanup for running by hand.
TRUNCATE entities, values, relations, spaces, entity_ranking_scores, entity_type_ranking,
         entity_topic_ranking, trending.agenda_listings CASCADE;
UPDATE trending.refresh_state SET last_full_pass_on = NULL WHERE id;
DROP TABLE ids, got;
