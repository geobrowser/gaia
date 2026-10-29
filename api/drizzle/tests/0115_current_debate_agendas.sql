-- Assertions for 0115: one API query for the current Debate agendas in a set of spaces (GEO-3503).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
--   1. Spaces: in the order asked (duplicates once), or with none asked every space with an agenda
--      this week, freshest first; personal spaces and spaces with nothing current return nothing.
--   2. The current agenda: the freshest arrived one dated this week; on the same Date the
--      later-created one (the larger id when created in the same second); current even before it holds a claim; none when the newest is older than a
--      week; a future-dated agenda, or one typed in a space without its Date there, is ignored.
--   3. Contents: block items only, sections and claims in the order Geo shows them (localeCompare:
--      case-insensitive, so Zz after b0; null positions last), each block once per agenda and each
--      claim once per block, numbered among what is returned; non-claims
--      and items written in another space left out; a section's name and its Topics in its space.
--   4. The agenda's own Claims and Topics as separate rows after its contents.
--   5. Earlier listings: other agendas in the window, once per claim per space at its newest listing
--      there, none on its own space's current agenda (so a space's rows don't depend on the others
--      asked, even when two asked spaces list the same claim); off by
--      default; the window clamped to 31 days, never an error.
--   6. listedAt, and lastListedOn: the claim's newest listing in any space, including a block item
--      added since the hourly sweep, the same on every row of a claim; null for topics.
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
--   S1..S5 DAO spaces (S3 has no agenda); SP a personal space.
--   ANEW, AOLD   S1, both dated today; ANEW created later, so it is current. AOLD is also typed an
--                agenda in S2, with no Date there.
--     ANEW  blocks BY (pos a0, and linked again at z9, "Topic Y", Topics TOPY) then BX (pos a1,
--           "Topic X", its Topics written in S2). BY: C5, PAGE (not a claim), C1. BX: C3 (pos a0, and again at a9), C5 (a5), C13
--           (b0, added after the hourly sweep), C4 (Zz), C18 (no position), C12 (written in S2).
--           Own Claims → C6 (twice), own Topics → TOPA. After the hourly sweep, BX also gets C10 (c0).
--     AOLD  C1, C2.
--   AY     S1, yesterday: C2, C3, C14, C7, C11 (C7 is on S2's current agenda; C11 is also an earlier
--          listing in S2).
--   ANEW also has a block BZ at pos 0, before BY, holding only PAGE, and names PAGE in its own Claims
--   (pos a), before C6: neither takes a number.
--   ATIE1, ATIE2  S6, both today and created in the same second: ATIE2 (the larger id) is current.     A10  S1, 10 days ago: C8.     AFUT  S1, tomorrow: C9.
--   AS2    S2, 2 days ago: C7, C10.        AS2Y S2, 3 days ago: C11.
--   AEMPTY S4, today, whose block holds only PAGE.   AS4Y  S4, yesterday: C16.
--   AS5    S5, 10 days ago: C17 (S5 has nothing this week).
--   AP     SP (personal), today: C15.
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE ids AS
SELECT k, ('b1140000-0000-4000-8000-' || lpad(to_hex(n), 12, '0'))::uuid AS id
FROM unnest(ARRAY['S1','S2','S3','S4','S5','S6','SP','ATIE1','ATIE2','BTIE1','BTIE2','BZ','C19','C20',
                  'ANEW','AOLD','AY','A10','AFUT','AS2','AS2Y','AEMPTY','AS4Y','AS5','AP',
                  'BY','BX','BOLD','BAY','BA10','BAFUT','BAS2','BAS2Y','BEMPTY','BAS4Y','BAS5','BAP',
                  'C1','C2','C3','C4','C5','C6','C7','C8','C9','C10','C11','C12','C13','C14','C15','C16','C17','C18',
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
CREATE OR REPLACE FUNCTION pg_temp.item(block_k text, claim_k text, space_k text, pos text) RETURNS void LANGUAGE sql AS $$
  SELECT pg_temp.rel(block_k, 'a99f9ce1-2ffa-4dac-8c61-f6310d46064a'::uuid, claim_k, space_k, pos) $$;
-- A block on agenda `agenda_k` at `pos`, with `claims` as its items in that order (positions a0, a1, …).
CREATE OR REPLACE FUNCTION pg_temp.block(agenda_k text, block_k text, space_k text, pos text, claims text[]) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE n integer := 0; c text;
BEGIN
  PERFORM pg_temp.rel(agenda_k, 'beaba5cb-a677-41a8-b353-77030613fc70'::uuid, block_k, space_k, pos);
  FOREACH c IN ARRAY claims LOOP
    PERFORM pg_temp.item(block_k, c, space_k, 'a' || n);
    n := n + 1;
  END LOOP;
END $$;

INSERT INTO spaces (id, type, address)
SELECT id, CASE WHEN k = 'SP' THEN 'Personal' ELSE 'DAO' END::"spaceTypes", 'addr-' || k
FROM ids WHERE k IN ('S1','S2','S3','S4','S5','S6','SP');

SELECT pg_temp.ent(k, '1780000000'), pg_temp.typed(k, 'CLAIM', 'S1') FROM ids WHERE k ~ '^C[0-9]+$';
SELECT pg_temp.ent(k, '1780000000') FROM ids WHERE k IN ('PAGE','TOPY','TOPA') OR k ~ '^B';
SELECT pg_temp.typed('PAGE', 'PAGET', 'S1');

SELECT pg_temp.agenda('AOLD', 'S1', 0, '1790000000');
SELECT pg_temp.block('AOLD', 'BOLD', 'S1', 'a0', ARRAY['C1','C2']);
SELECT pg_temp.typed('AOLD', 'AGENDA', 'S2');

SELECT pg_temp.agenda('ANEW', 'S1', 0, '1790000500');
SELECT pg_temp.rel('ANEW', 'beaba5cb-a677-41a8-b353-77030613fc70'::uuid, 'BX', 'S1', 'a1');
SELECT pg_temp.block('ANEW', 'BY', 'S1', 'a0', ARRAY['C5','PAGE','C1']);
SELECT pg_temp.item('BX', 'C3', 'S1', 'a0'), pg_temp.item('BX', 'C3', 'S1', 'a9'), pg_temp.item('BX', 'C4', 'S1', 'Zz'),
       pg_temp.item('BX', 'C5', 'S1', 'a5'), pg_temp.item('BX', 'C12', 'S2', 'a6'), pg_temp.item('BX', 'C18', 'S1', NULL);
SELECT pg_temp.name('BY', 'S1', 'Topic Y'), pg_temp.name('BX', 'S1', 'Topic X');
SELECT pg_temp.rel('BY', '806d52bc-27e9-4c91-93c0-57978b093351'::uuid, 'TOPY', 'S1');
SELECT pg_temp.rel('BX', '806d52bc-27e9-4c91-93c0-57978b093351'::uuid, 'TOPY', 'S2');
SELECT pg_temp.rel('ANEW', 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid, 'C6', 'S1');
SELECT pg_temp.rel('ANEW', 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid, 'C6', 'S1', 'b0');
SELECT pg_temp.rel('ANEW', 'beaba5cb-a677-41a8-b353-77030613fc70'::uuid, 'BY', 'S1', 'z9');
SELECT pg_temp.block('ANEW', 'BZ', 'S1', '0', ARRAY['PAGE']);
SELECT pg_temp.rel('ANEW', 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid, 'PAGE', 'S1', 'a');
SELECT pg_temp.rel('ANEW', '806d52bc-27e9-4c91-93c0-57978b093351'::uuid, 'TOPA', 'S1');

SELECT pg_temp.agenda('AY', 'S1', 1, '1790000000'),     pg_temp.block('AY', 'BAY', 'S1', 'a0', ARRAY['C2','C3','C14','C7','C11']);
SELECT pg_temp.agenda('A10', 'S1', 10, '1790000000'),   pg_temp.block('A10', 'BA10', 'S1', 'a0', ARRAY['C8']);
SELECT pg_temp.agenda('AFUT', 'S1', -1, '1790000000'),  pg_temp.block('AFUT', 'BAFUT', 'S1', 'a0', ARRAY['C9']);
SELECT pg_temp.agenda('AS2', 'S2', 2, '1790000000'),    pg_temp.block('AS2', 'BAS2', 'S2', 'a0', ARRAY['C7','C10']);
SELECT pg_temp.agenda('AS2Y', 'S2', 3, '1790000000'),   pg_temp.block('AS2Y', 'BAS2Y', 'S2', 'a0', ARRAY['C11']);
SELECT pg_temp.agenda('AEMPTY', 'S4', 0, '1790000000'), pg_temp.block('AEMPTY', 'BEMPTY', 'S4', 'a0', ARRAY['PAGE']);
SELECT pg_temp.agenda('AS4Y', 'S4', 1, '1790000000'),   pg_temp.block('AS4Y', 'BAS4Y', 'S4', 'a0', ARRAY['C16']);
SELECT pg_temp.agenda('AS5', 'S5', 10, '1790000000'),   pg_temp.block('AS5', 'BAS5', 'S5', 'a0', ARRAY['C17']);
SELECT pg_temp.agenda('ATIE1', 'S6', 0, '1790000000'), pg_temp.block('ATIE1', 'BTIE1', 'S6', 'a0', ARRAY['C19']);
SELECT pg_temp.agenda('ATIE2', 'S6', 0, '1790000000'), pg_temp.block('ATIE2', 'BTIE2', 'S6', 'a0', ARRAY['C20']);
SELECT pg_temp.agenda('AP', 'SP', 0, '1790000000'),     pg_temp.block('AP', 'BAP', 'SP', 'a0', ARRAY['C15']);

-- The hourly sweep fills the listings table; then a block item arrives that it has not seen.
SELECT trending.refresh_agenda_listings(true);
SELECT pg_temp.item('BX', 'C13', 'S1', 'b0'), pg_temp.item('BX', 'C10', 'S1', 'c0');

CREATE OR REPLACE FUNCTION pg_temp.k(id uuid) RETURNS text LANGUAGE sql AS $$ SELECT k FROM ids WHERE ids.id = $1 $$;
CREATE TEMP TABLE got AS
SELECT row_number() OVER () AS n, r.*
FROM public.current_debate_agendas(ARRAY[pg_temp.i('S2'), pg_temp.i('S1'), pg_temp.i('SP'), pg_temp.i('S3'),
                                         pg_temp.i('S4'), pg_temp.i('S5')], 6) r;
-- The keys of the rows matching a filter, in returned order.
CREATE OR REPLACE FUNCTION pg_temp.seq(q text) RETURNS text[] LANGUAGE plpgsql AS $$
DECLARE out text[];
BEGIN
  EXECUTE 'SELECT array_agg(pg_temp.k(entity_id) ORDER BY n) FROM got WHERE ' || q INTO out;
  RETURN out;
END $$;
-- The space keys of a call's rows, in returned order, once each.
CREATE OR REPLACE FUNCTION pg_temp.spaces_of(space_ids uuid[], earlier integer) RETURNS text[] LANGUAGE sql AS $$
  SELECT array_agg(k ORDER BY first_n) FROM (
    SELECT pg_temp.k(r.space_id) AS k, min(r.n) AS first_n
    FROM (SELECT row_number() OVER () AS n, x.space_id FROM public.current_debate_agendas(space_ids, earlier) x) r
    GROUP BY r.space_id) s $$;

-- ---------------------------------------------------------------------------
-- 1. Spaces.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.spaces_of(ARRAY[pg_temp.i('S2'), pg_temp.i('S1'), pg_temp.i('SP'), pg_temp.i('S3'),
                                      pg_temp.i('S4'), pg_temp.i('S5')], 6) = ARRAY['S2','S1','S4'],
              '1: spaces in the order asked; nothing for a personal space, a space with no agenda, or one with none this week');
SELECT assert((SELECT count(*) FROM public.current_debate_agendas(ARRAY[pg_temp.i('S1'), pg_temp.i('S2'), pg_temp.i('S1')], 6))
              = (SELECT count(*) FROM public.current_debate_agendas(ARRAY[pg_temp.i('S1'), pg_temp.i('S2')], 6))
              AND pg_temp.spaces_of(ARRAY[pg_temp.i('S1'), pg_temp.i('S2'), pg_temp.i('S1')], 6) = ARRAY['S1','S2'],
              '1: a space asked twice comes back once, at its first place');
SELECT assert(pg_temp.spaces_of(NULL, 0) = ARRAY['S1','S6','S2'],
              '1: no spaces asked: every space with an agenda this week, freshest current agenda first');
SELECT assert(pg_temp.spaces_of(NULL, 31) = ARRAY['S1','S4','S6','S2'],
              '1: and a longer earlier window adds no space with nothing this week (not S5)');
SELECT assert(NOT EXISTS (SELECT 1 FROM public.current_debate_agendas('{}'::uuid[], 6)),
              '1: an empty list asks for nothing');

-- ---------------------------------------------------------------------------
-- 2. The current agenda.
-- ---------------------------------------------------------------------------
SELECT assert((SELECT array_agg(DISTINCT pg_temp.k(agenda_id)) FROM got WHERE is_current AND space_id = pg_temp.i('S1')) = ARRAY['ANEW'],
              '2: of two agendas dated today, the later-created one is current');
SELECT assert((SELECT array_agg(DISTINCT pg_temp.k(agenda_id)) FROM got WHERE is_current AND space_id = pg_temp.i('S2')) = ARRAY['AS2']
              AND (SELECT DISTINCT listed_on FROM got WHERE is_current AND space_id = pg_temp.i('S2')) = pg_temp.today() - 2,
              '2: a space''s freshest agenda this week is current even when it is not today''s');
SELECT assert(NOT EXISTS (SELECT 1 FROM got WHERE space_id = pg_temp.i('S4') AND is_current)
              AND pg_temp.seq('space_id = pg_temp.i(''S4'')') = ARRAY['C16'],
              '2: today''s agenda with no claim yet is current, so yesterday''s claims are earlier ones');
SELECT assert((SELECT array_agg(DISTINCT pg_temp.k(r.agenda_id)) FROM public.current_debate_agendas(ARRAY[pg_temp.i('S6')]) r) = ARRAY['ATIE2'],
              '2: two agendas on the same Date created in the same second: the larger id is current');
SELECT assert(NOT EXISTS (SELECT 1 FROM public.current_debate_agendas(ARRAY[pg_temp.i('S5')], 31) r WHERE r.is_current),
              '2: an agenda older than a week is never current');
SELECT assert(NOT EXISTS (SELECT 1 FROM got WHERE agenda_id = pg_temp.i('AFUT') OR entity_id = pg_temp.i('C9')),
              '2: a future-dated agenda is ignored');
SELECT assert(NOT EXISTS (SELECT 1 FROM got WHERE agenda_id = pg_temp.i('AOLD') AND space_id = pg_temp.i('S2')),
              '2: an agenda typed in a space without its Date there is not that space''s');

-- ---------------------------------------------------------------------------
-- 3. Contents.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.seq('is_current AND source = ''block_item'' AND space_id = pg_temp.i(''S1'')')
              = ARRAY['C5','C1','C3','C5','C13','C10','C4','C18'],
              '3: sections then claims in Geo''s order (Zz after b0, no position last), each claim once per block');
SELECT assert((SELECT count(DISTINCT section_position) FROM got WHERE section_id = pg_temp.i('BY')) = 1
              AND (SELECT max(section_position) FROM got WHERE agenda_id = pg_temp.i('ANEW')) = 2,
              '3: a block linked twice is one section, at its first position');
SELECT assert((SELECT item_position FROM got WHERE section_id = pg_temp.i('BY') AND entity_id = pg_temp.i('C1')) = 2
              AND NOT EXISTS (SELECT 1 FROM got WHERE section_id = pg_temp.i('BZ'))
              AND (SELECT item_position FROM got WHERE source = 'agenda_claims' AND entity_id = pg_temp.i('C6')) = 1,
              '3: positions number only what is returned: a non-claim item, a block with no claim and a non-claim lever take no number');
SELECT assert((SELECT item_position FROM got WHERE section_id = pg_temp.i('BX') AND entity_id = pg_temp.i('C3')) = 1,
              '3: a claim in a block twice is at its first position');
SELECT assert(NOT EXISTS (SELECT 1 FROM got WHERE entity_id IN (pg_temp.i('PAGE'), pg_temp.i('C12'))),
              '3: a non-claim and an item written in another space are left out');
SELECT assert((SELECT bool_and(section_name = 'Topic Y' AND section_topic_id = pg_temp.i('TOPY') AND section_position = 1)
               FROM got WHERE section_id = pg_temp.i('BY')),
              '3: the first section carries its block, name and topic');
SELECT assert((SELECT bool_and(section_name = 'Topic X' AND section_topic_id IS NULL AND section_position = 2)
               FROM got WHERE section_id = pg_temp.i('BX')),
              '3: a section''s Topics written in another space is not its topic');

-- ---------------------------------------------------------------------------
-- 4. Ranking levers.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.seq('source <> ''block_item''') = ARRAY['C6','TOPA'],
              '4: the agenda''s own Claims (C6 once, though named twice) then Topics, as their own sources');
SELECT assert((SELECT bool_and(section_id IS NULL) FROM got WHERE source <> 'block_item')
              AND (SELECT min(n) FROM got WHERE source <> 'block_item' AND space_id = pg_temp.i('S1'))
                > (SELECT max(n) FROM got WHERE is_current AND source = 'block_item' AND space_id = pg_temp.i('S1')),
              '4: after the agenda''s contents, with no section');

-- ---------------------------------------------------------------------------
-- 5. Earlier listings.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.seq('NOT is_current AND space_id = pg_temp.i(''S1'')') = ARRAY['C2','C14','C7','C11']
              AND pg_temp.seq('NOT is_current AND space_id = pg_temp.i(''S2'')') = ARRAY['C11'],
              '5: each space''s other agendas in the window, newest first (AOLD today, then AY)');
SELECT assert((SELECT array_agg(pg_temp.k(entity_id) ORDER BY n) FROM (
                 SELECT row_number() OVER () AS n, r.* FROM public.current_debate_agendas(ARRAY[pg_temp.i('S2'), pg_temp.i('S1')], 1) r) x
               WHERE NOT x.is_current) = ARRAY['C2','C14','C7','C11'],
              '5: a shorter window keeps only listings inside it (not S2''s, 3 days old)');
SELECT assert((SELECT array_agg(pg_temp.k(r.entity_id) ORDER BY r.n) FROM (
                 SELECT row_number() OVER () AS n, x.* FROM public.current_debate_agendas(ARRAY[pg_temp.i('S1')], 6) x) r
               WHERE NOT r.is_current) = ARRAY['C2','C14','C7','C11'],
              '5: a space''s earlier rows are the same asked alone (C7 is on S2''s current agenda, C11 earlier in S2 too)');
SELECT assert((SELECT count(*) FROM got WHERE entity_id = pg_temp.i('C2')) = 1
              AND (SELECT listed_on FROM got WHERE entity_id = pg_temp.i('C2')) = pg_temp.today(),
              '5: once per claim, at its newest listing');
SELECT assert(NOT EXISTS (SELECT 1 FROM got WHERE NOT is_current AND entity_id IN (pg_temp.i('C1'), pg_temp.i('C3'), pg_temp.i('C8'))),
              '5: none that is on a current agenda, and none older than the window');
SELECT assert(NOT EXISTS (SELECT 1 FROM public.current_debate_agendas(ARRAY[pg_temp.i('S1')]) r WHERE NOT r.is_current)
              AND NOT EXISTS (SELECT 1 FROM public.current_debate_agendas(ARRAY[pg_temp.i('S1')], NULL) r WHERE NOT r.is_current)
              AND NOT EXISTS (SELECT 1 FROM public.current_debate_agendas(ARRAY[pg_temp.i('S1')], -5) r WHERE NOT r.is_current),
              '5: off by default, and for a null or negative window');
SELECT assert((SELECT array_agg(pg_temp.k(entity_id) ORDER BY entity_id)
               FROM public.current_debate_agendas(ARRAY[pg_temp.i('S1'), pg_temp.i('S5')], 2147483647) r
               WHERE NOT r.is_current AND r.entity_id IN (pg_temp.i('C8'), pg_temp.i('C17')))
              = (SELECT array_agg(pg_temp.k(id) ORDER BY id) FROM ids WHERE k IN ('C8','C17')),
              '5: a huge window is 31 days, not an error, and reaches past the week');

-- ---------------------------------------------------------------------------
-- 6. listedAt and lastListedOn.
-- ---------------------------------------------------------------------------
SELECT assert((SELECT bool_and(listed_at = (pg_temp.today()::timestamp AT TIME ZONE 'America/New_York'))
               FROM got WHERE agenda_id = pg_temp.i('ANEW')),
              '6: listedAt is the agenda Date''s midnight in New York');
SELECT assert((SELECT last_listed_on FROM got WHERE entity_id = pg_temp.i('C13')) = pg_temp.today(),
              '6: a block item the hourly sweep has not seen yet is listed today');
SELECT assert((SELECT last_listed_on FROM got WHERE entity_id = pg_temp.i('C14')) = pg_temp.today() - 1
              AND (SELECT array_agg(last_listed_on) FROM got WHERE entity_id = pg_temp.i('C11')) = ARRAY[pg_temp.today() - 1, pg_temp.today() - 1],
              '6: otherwise the claim''s newest listing');
SELECT assert((SELECT array_agg(last_listed_on) FROM got WHERE entity_id = pg_temp.i('C10')) = ARRAY[pg_temp.today(), pg_temp.today()],
              '6: the same on every row of a claim: C10 on S2''s agenda of 2 days ago reads today, from S1''s new listing');
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
              = ARRAY['agenda_claims','agenda_topics','block_item','block_item','block_item','block_item','block_item','block_item','block_item'],
              '7: block items (C1 C3 C4 C5 C10 C13 C18, not PAGE or C12), its Claims and its Topics');

-- Cleanup for running by hand.
TRUNCATE entities, values, relations, spaces, entity_ranking_scores, entity_type_ranking,
         entity_topic_ranking, trending.agenda_listings CASCADE;
UPDATE trending.refresh_state SET last_full_pass_on = NULL WHERE id;
DROP TABLE ids, got;
