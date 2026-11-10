-- Assertions for 0122: the order of an agenda's Debates relation orders its debates at the top of Best
-- (GEO-3606).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts. Expected values are
-- written against entity_ranking_config, so retuning the defaults does not need a new suite.
--
--   1. Places: the positions table holds each named debate's place, numbered over the targets that
--      count and carry a relation position, the same as currentDebateAgendas' itemPosition. Links with
--      no position take no place.
--   2. On the agenda's Date, each place adds step × (slots − place + 1) to the listing boost, so the
--      agenda's order beats an organically stronger debate; the term fades by the day factor after it.
--      Unordered debates keep the plain boost. Yesterday's place carries through an unordered agenda
--      today; today's place replaces yesterday's.
--   3. Only named debates: a debate on a listed claim and the claim itself are unchanged. Named by two
--      agendas the same day, a debate takes its best place. Past the slots, only the 0118 boost.
--   4. A vote's re-score keeps the term. Reordering the Debates relation re-scores just the debates
--      whose place moved, and their order follows. A day factor of 0 makes it today only.
--   5. A step of 0 turns it off: no term and no re-scores for moves, while the table stays in step.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

TRUNCATE entities, values, relations, votes_count, user_votes, spaces, entity_ranking_scores,
         entity_type_weights, entity_type_exclusions, entity_type_ranking, entity_topic_ranking,
         entity_feed_blocklist, personalization.feed_item_signals_hourly, trending.agenda_listings,
         trending.agenda_debate_positions
         CASCADE;
UPDATE trending.refresh_state SET last_full_pass_on = NULL WHERE id;

UPDATE entity_ranking_config
   SET participation_weight = 4, participation_cap = 19, comment_weight = 0, comment_cap = 30,
       tau_seconds = 100000, trending_boost = 8, trending_topic_boost = 2.5, trending_day_factor = 0.65,
       trending_debate_factor = 0.75, trending_debate_window_days = 7, trending_horizon_days = 16,
       trending_position_step = 30, trending_position_slots = 20, trending_position_day_factor = 0.65
 WHERE id;

CREATE OR REPLACE FUNCTION pg_temp.b() RETURNS numeric LANGUAGE sql AS $$
  SELECT trending_boost FROM entity_ranking_config WHERE id $$;
CREATE OR REPLACE FUNCTION pg_temp.f(days integer) RETURNS numeric LANGUAGE sql AS $$
  SELECT power(trending_day_factor, days) FROM entity_ranking_config WHERE id $$;
-- The position term at `place`, `days` after the listing.
CREATE OR REPLACE FUNCTION pg_temp.p(place integer, days integer DEFAULT 0) RETURNS numeric LANGUAGE sql AS $$
  SELECT CASE WHEN place > trending_position_slots THEN 0
              ELSE trending_position_step * (trending_position_slots - place + 1)
                   * power(trending_position_day_factor, days) END
  FROM entity_ranking_config WHERE id $$;

-- ---------------------------------------------------------------------------
-- Fixtures. Days are New York calendar days relative to today.
--   UNL     an unlisted claim. D1 D2 D3 DM, 30 days old, and DSTRONG, created today with 40 votes,
--           are debates on it. A0 (S1, today) names D1 D2 DSTRONG DM at a0 a1 a2 a3, and NOTD (a
--           claim, which takes no place) at a05, between D1 and D2.
--           DNULL is also named by A0, with no position; DDEM is named last (a4).
--   A0B     an agenda in S2 today naming DM first.
--   AU      an agenda in S2 today naming DU1, DU2 and DY2 with no positions at all.
--   AY      yesterday's agenda in S1, placing DY, DY2 and DDEM first, second and third.
--   All of these debates are 30 days old except DSTRONG.
--   LC      a claim on A0's block, with debate DLC (10 days old): lifted through its claim only.
--   ORG     an organic debate on UNL created today, with 40 votes, named by nothing.
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE ids AS
SELECT k, ('b1220000-0000-4000-8000-' || lpad(to_hex(n), 12, '0'))::uuid AS id
FROM unnest(ARRAY['UNL','LC','NOTD','D1','D2','D3','DM','DSTRONG','DY','DLC','ORG',
                  'DNULL','DDEM','DU1','DU2','DY2',
                  'A0','A0B','AU','AY','S1','S2']) WITH ORDINALITY AS u(k, n)
UNION ALL SELECT * FROM (VALUES
  ('DEBATE', 'fd51f935-2063-4617-be39-7b672b23364c'::uuid),
  ('CLAIM',  '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid),
  ('AGENDA', '79e3757b-2630-492c-9d3b-d73a78d45893'::uuid)) AS v(k, id);
CREATE OR REPLACE FUNCTION pg_temp.i(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM ids WHERE ids.k = $1 $$;
CREATE OR REPLACE FUNCTION pg_temp.today() RETURNS date LANGUAGE sql AS $$
  SELECT (now() AT TIME ZONE 'America/New_York')::date $$;
CREATE OR REPLACE FUNCTION pg_temp.noon(d integer) RETURNS bigint LANGUAGE sql AS $$
  SELECT extract(epoch FROM ((pg_temp.today() - d)::timestamp + interval '12 hours') AT TIME ZONE 'America/New_York')::bigint $$;
CREATE OR REPLACE FUNCTION pg_temp.midnight(d integer) RETURNS bigint LANGUAGE sql AS $$
  SELECT extract(epoch FROM (pg_temp.today() - d)::timestamp AT TIME ZONE 'America/New_York')::bigint $$;

CREATE OR REPLACE FUNCTION pg_temp.ent(k text, created bigint) RETURNS void LANGUAGE sql AS $$
  INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
  VALUES (pg_temp.i(k), created::text, '0', created::text, '0');
  INSERT INTO values (id, entity_id, property_id, space_id, text)
  VALUES (gen_random_uuid()::text, pg_temp.i(k), 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid, pg_temp.i('S1'), 'fixture ' || k);
$$;
CREATE OR REPLACE FUNCTION pg_temp.rel(from_k text, type_id uuid, to_k text, space_k text) RETURNS void LANGUAGE sql AS $$
  INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
  VALUES (gen_random_uuid(), gen_random_uuid(), type_id, pg_temp.i(from_k), pg_temp.i(to_k), pg_temp.i(space_k), false)
$$;
CREATE OR REPLACE FUNCTION pg_temp.typed(k text, type_k text) RETURNS void LANGUAGE sql AS $$
  SELECT pg_temp.rel(k, '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid, type_k, 'S1') $$;
CREATE OR REPLACE FUNCTION pg_temp.names(agenda_k text, debate_k text, space_k text) RETURNS void LANGUAGE sql AS $$
  SELECT pg_temp.rel(agenda_k, 'b96bf701-a399-430d-a072-f5e910cdeda9'::uuid, debate_k, space_k) $$;
CREATE OR REPLACE FUNCTION pg_temp.unname(agenda_k text, debate_k text) RETURNS void LANGUAGE sql AS $$
  DELETE FROM relations WHERE from_entity_id = pg_temp.i(agenda_k) AND to_entity_id = pg_temp.i(debate_k)
    AND type_id = 'b96bf701-a399-430d-a072-f5e910cdeda9'::uuid $$;
-- An agenda in `space_k` dated `d` days ago (negative: the future), one block listing `claims`.
CREATE OR REPLACE FUNCTION pg_temp.agenda(k text, space_k text, d integer, claims text[]) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE block uuid := gen_random_uuid(); c text;
BEGIN
  INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
  VALUES (pg_temp.i(k), '1790000000', '0', '1790000000', '0');
  PERFORM pg_temp.rel(k, '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid, 'AGENDA', space_k);
  INSERT INTO values (id, entity_id, property_id, space_id, datetime, datetime_utc)
  VALUES (gen_random_uuid()::text, pg_temp.i(k), '52665f3e-fb7d-48d5-8b6b-abb21b0d36db'::uuid, pg_temp.i(space_k),
          'x', to_timestamp(pg_temp.midnight(d)));
  INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
  VALUES (gen_random_uuid(), gen_random_uuid(), 'beaba5cb-a677-41a8-b353-77030613fc70'::uuid, pg_temp.i(k), block, pg_temp.i(space_k), false);
  FOREACH c IN ARRAY claims LOOP
    INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
    VALUES (gen_random_uuid(), gen_random_uuid(), 'a99f9ce1-2ffa-4dac-8c61-f6310d46064a'::uuid, block, pg_temp.i(c), pg_temp.i(space_k), false);
  END LOOP;
END $$;
CREATE OR REPLACE FUNCTION pg_temp.debate(k text, claim_k text, d integer) RETURNS void LANGUAGE sql AS $$
  SELECT pg_temp.ent(k, pg_temp.noon(d));
  SELECT pg_temp.typed(k, 'DEBATE');
  SELECT pg_temp.rel(k, 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid, claim_k, 'S1');
$$;
CREATE OR REPLACE FUNCTION pg_temp.votes(k text, n integer) RETURNS void LANGUAGE sql AS $$
  INSERT INTO votes_count (object_id, object_type, space_id, vote_kind, positive, negative)
  VALUES (pg_temp.i(k), 0, pg_temp.i('S1'), 1, n, 0) $$;

CREATE OR REPLACE FUNCTION pg_temp.score(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT ranking_score FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k) $$;
CREATE OR REPLACE FUNCTION pg_temp.trend(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT COALESCE((SELECT trending_score FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k)), 0) $$;
CREATE OR REPLACE FUNCTION pg_temp.credit(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT COALESCE((SELECT age_credit FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k)), 0) $$;
CREATE OR REPLACE FUNCTION pg_temp.src(k text) RETURNS text LANGUAGE sql AS $$
  SELECT trending_source FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k) $$;
CREATE OR REPLACE FUNCTION pg_temp.listed_on(k text) RETURNS date LANGUAGE sql AS $$
  SELECT public.entities_last_listed_on(e) FROM entities e WHERE e.id = pg_temp.i(k) $$;
CREATE OR REPLACE FUNCTION pg_temp.near(a numeric, b numeric) RETURNS boolean LANGUAGE sql AS $$
  SELECT abs(a - b) < 1e-9 $$;

CREATE OR REPLACE FUNCTION pg_temp.place(agenda_k text, k text, pos text) RETURNS void LANGUAGE sql AS $$
  UPDATE relations SET position = pos
  WHERE from_entity_id = pg_temp.i(agenda_k) AND to_entity_id = pg_temp.i(k)
    AND type_id = 'b96bf701-a399-430d-a072-f5e910cdeda9'::uuid $$;
CREATE OR REPLACE FUNCTION pg_temp.stored_place(agenda_k text, k text) RETURNS integer LANGUAGE sql AS $$
  SELECT position FROM trending.agenda_debate_positions
  WHERE agenda_id = pg_temp.i(agenda_k) AND entity_id = pg_temp.i(k) $$;

INSERT INTO spaces (id, type, address)
SELECT id, 'DAO'::"spaceTypes", 'addr-' || k FROM ids WHERE k IN ('S1','S2');

SELECT pg_temp.ent(k, pg_temp.noon(40)), pg_temp.typed(k, 'CLAIM') FROM ids WHERE k IN ('UNL','LC','NOTD');

SELECT pg_temp.agenda('A0', 'S1', 0, ARRAY['LC']);
SELECT pg_temp.agenda('A0B', 'S2', 0, ARRAY[]::text[]);
SELECT pg_temp.agenda('AU', 'S2', 0, ARRAY[]::text[]);
SELECT pg_temp.agenda('AY', 'S1', 1, ARRAY[]::text[]);

SELECT pg_temp.debate(k, 'UNL', 30) FROM ids WHERE k IN ('D1','D2','D3','DM','DY','DNULL','DDEM','DU1','DU2','DY2');
SELECT pg_temp.debate('DSTRONG', 'UNL', 0), pg_temp.debate('ORG', 'UNL', 0), pg_temp.debate('DLC', 'LC', 10);
SELECT pg_temp.votes('DSTRONG', 40), pg_temp.votes('ORG', 40);

SELECT pg_temp.names('A0', k, 'S1') FROM ids WHERE k IN ('D1','NOTD','D2','DSTRONG','DM','DNULL','DDEM');
SELECT pg_temp.place('A0', 'D1', 'a0'), pg_temp.place('A0', 'NOTD', 'a05'), pg_temp.place('A0', 'D2', 'a1'),
       pg_temp.place('A0', 'DSTRONG', 'a2'), pg_temp.place('A0', 'DM', 'a3'), pg_temp.place('A0', 'DDEM', 'a4');
SELECT pg_temp.names('A0B', 'DM', 'S2'), pg_temp.place('A0B', 'DM', 'a0');
SELECT pg_temp.names('AU', k, 'S2') FROM ids WHERE k IN ('DU1','DU2','DY2');
SELECT pg_temp.names('AY', k, 'S1') FROM ids WHERE k IN ('DY','DY2','DDEM');
SELECT pg_temp.place('AY', 'DY', 'a0'), pg_temp.place('AY', 'DY2', 'a1'), pg_temp.place('AY', 'DDEM', 'a2');

SELECT trending.refresh_agenda_listings(false);
SELECT public.refresh_entity_ranking_scores(ARRAY[pg_temp.i('ORG'), pg_temp.i('D3')]);

-- ---------------------------------------------------------------------------
-- 1. Places.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.stored_place('A0', 'D1') = 1 AND pg_temp.stored_place('A0', 'D2') = 2
              AND pg_temp.stored_place('A0', 'DSTRONG') = 3 AND pg_temp.stored_place('A0', 'DM') = 4
              AND pg_temp.stored_place('A0', 'DDEM') = 5
              AND pg_temp.stored_place('A0B', 'DM') = 1 AND pg_temp.stored_place('AY', 'DY') = 1,
              '1: each named debate''s place, numbered over the debates only (NOTD takes none)');
SELECT assert(pg_temp.stored_place('A0', 'DNULL') IS NULL
              AND NOT EXISTS (SELECT 1 FROM trending.agenda_debate_positions WHERE agenda_id = pg_temp.i('AU')),
              '1: a link with no position takes no place, in a partly ordered list or an unordered one');
SELECT assert(NOT EXISTS (SELECT 1 FROM trending.agenda_debate_positions WHERE entity_id = pg_temp.i('NOTD'))
              AND NOT EXISTS (SELECT 1 FROM trending.agenda_debate_positions WHERE entity_id = pg_temp.i('DLC')),
              '1: no place for a non-debate target or a debate lifted through its claim');
SELECT assert(NOT EXISTS (
                SELECT p.entity_id, p.position FROM trending.agenda_debate_positions p WHERE p.agenda_id = pg_temp.i('A0')
                EXCEPT
                SELECT c.entity_id, c.item_position FROM public.current_debate_agendas(ARRAY[pg_temp.i('S1')]) c
                WHERE c.source = 'agenda_debates'),
              '1: the places are currentDebateAgendas'' itemPositions (ordered links come first)');

-- ---------------------------------------------------------------------------
-- 2. The term.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.trend('D1') = pg_temp.b() + pg_temp.p(1) AND pg_temp.trend('D2') = pg_temp.b() + pg_temp.p(2)
              AND pg_temp.trend('DSTRONG') = pg_temp.b() + pg_temp.p(3) AND pg_temp.src('D1') = 'listing',
              '2: on the Date, each place adds its steps to the listing boost');
SELECT assert(pg_temp.score('ORG') > pg_temp.score('D1') - pg_temp.trend('D1'),
              '2: (the fixture: without the agenda, the organic debate outranks D1)');
SELECT assert(pg_temp.score('D1') > pg_temp.score('D2') AND pg_temp.score('D2') > pg_temp.score('DSTRONG')
              AND pg_temp.score('DSTRONG') > pg_temp.score('ORG'),
              '2: so Best follows the agenda''s order, a newer debate with 40 votes included, above the organic one');
SELECT assert(pg_temp.near(pg_temp.trend('DY'), pg_temp.b() * pg_temp.f(1) + pg_temp.p(1, 1))
              AND pg_temp.p(1, 1) > 0 AND pg_temp.p(1, 1) < pg_temp.p(1),
              '2: yesterday''s first place fades by the day factor, it does not lapse');
SELECT assert(pg_temp.score('DY') > pg_temp.score('D3') + pg_temp.b(),
              '2: so with no agenda today, yesterday''s order still leads');
SELECT assert(pg_temp.trend('DNULL') = pg_temp.b() AND pg_temp.trend('DU1') = pg_temp.b() AND pg_temp.trend('DU2') = pg_temp.b(),
              '2: debates named with no order keep the plain boost, in their organic order');
SELECT assert(pg_temp.near(pg_temp.trend('DY2'), pg_temp.b() + pg_temp.p(2, 1)),
              '2: named today with no order, a debate keeps yesterday''s place, faded');
SELECT assert(pg_temp.trend('DDEM') = pg_temp.b() + pg_temp.p(5),
              '2: placed today, today''s place replaces yesterday''s');
SELECT assert(pg_temp.trend('D3') = 0, '2: an unnamed debate gets nothing');

-- ---------------------------------------------------------------------------
-- 3. Only named debates; best place; the slots.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.trend('DLC') = pg_temp.b(), '3: a debate lifted through its listed claim keeps the plain boost');
SELECT assert(pg_temp.trend('LC') <= pg_temp.b() AND pg_temp.trend('UNL') = 0,
              '3: and claims take no position term');
SELECT assert(pg_temp.trend('DM') = pg_temp.b() + pg_temp.p(1),
              '3: named fourth in S1 and first in S2 the same day: its best place');
UPDATE entity_ranking_config SET trending_position_slots = 2 WHERE id;
SELECT trending.refresh_agenda_listings(true);
SELECT assert(pg_temp.trend('D1') = pg_temp.b() + pg_temp.p(1) AND pg_temp.trend('D2') = pg_temp.b() + pg_temp.p(2)
              AND pg_temp.trend('DSTRONG') = pg_temp.b(),
              '3: past the slots, only the 0118 boost');
UPDATE entity_ranking_config SET trending_position_slots = 20 WHERE id;
SELECT trending.refresh_agenda_listings(true);

-- ---------------------------------------------------------------------------
-- 4. Re-scores, reordering, a day factor.
-- ---------------------------------------------------------------------------
SELECT pg_temp.votes('D2', 3);
SELECT public.refresh_entity_ranking_scores(ARRAY[pg_temp.i('D2')]);
SELECT assert(pg_temp.trend('D2') = pg_temp.b() + pg_temp.p(2), '4: a vote''s re-score keeps the term');

-- DSTRONG to the top: D1 D2 DSTRONG DM becomes DSTRONG D1 D2 DM. Three places move; DM's does not.
SELECT pg_temp.place('A0', 'DSTRONG', '0');
SELECT assert(trending.refresh_agenda_listings(false) = 3, '4: an hourly run re-scores just the debates whose place moved');
SELECT assert(pg_temp.stored_place('A0', 'DSTRONG') = 1 AND pg_temp.stored_place('A0', 'D1') = 2
              AND pg_temp.stored_place('A0', 'D2') = 3,
              '4: the table follows the new order');
SELECT assert(pg_temp.score('DSTRONG') > pg_temp.score('D1') AND pg_temp.score('D1') > pg_temp.score('D2'),
              '4: and so does Best');
SELECT assert(trending.refresh_agenda_listings(false) = 0, '4: an unchanged hour re-scores nothing');

UPDATE entity_ranking_config SET trending_position_day_factor = 0 WHERE id;
SELECT trending.refresh_agenda_listings(true);
SELECT assert(pg_temp.near(pg_temp.trend('DY'), pg_temp.b() * pg_temp.f(1)) AND pg_temp.trend('D1') = pg_temp.b() + pg_temp.p(2),
              '4: a day factor of 0 makes the order today only');
UPDATE entity_ranking_config SET trending_position_day_factor = 0.65 WHERE id;

-- ---------------------------------------------------------------------------
-- 5. Off.
-- ---------------------------------------------------------------------------
UPDATE entity_ranking_config SET trending_position_step = 0 WHERE id;
SELECT trending.refresh_agenda_listings(true);
SELECT assert(pg_temp.trend('DSTRONG') = pg_temp.b() AND pg_temp.trend('D1') = pg_temp.b(),
              '5: a step of 0 leaves the plain boost');
SELECT pg_temp.place('A0', 'DSTRONG', 'z');
SELECT assert(trending.refresh_agenda_listings(false) = 0, '5: and a move re-scores nothing');
SELECT assert(pg_temp.stored_place('A0', 'DSTRONG') = 5 AND pg_temp.stored_place('A0', 'D1') = 1,
              '5: while the table stays in step');

-- Cleanup for running by hand.
UPDATE entity_ranking_config SET trending_position_step = 30 WHERE id;
UPDATE trending.refresh_state SET last_full_pass_on = NULL WHERE id;
TRUNCATE entities, values, relations, votes_count, spaces, entity_ranking_scores, entity_type_ranking,
         entity_topic_ranking, trending.agenda_listings, trending.agenda_debate_positions CASCADE;
DROP TABLE ids;
