-- Assertions for 0123: an agenda in the global agenda space (the Debates space) sets the order at the top of
-- the whole feed (GEO-3631).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts. Expected values are
-- written against entity_ranking_config.
--
--   1. A place on the global agenda adds the global base to 0122's term, so its last place outranks every
--      space agenda's first place, in the global agenda's order.
--   2. A debate placed by both the global agenda and its space agenda the same day takes its global place.
--      Space agendas still order their own debates. Yesterday's global place fades with the term.
--   3. With no global space, or a global base of 0, Best is 0122's.
--   4. With only the global base on, a move across the slots re-scores within the hour.

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
       trending_position_base = 2500, trending_position_step = 100, trending_position_slots = 50,
       trending_position_day_factor = 0.3, trending_position_global_base = 10000,
       trending_global_agenda_space_id = 'b1230000-0000-4000-8000-00000000000d'
 WHERE id;

CREATE OR REPLACE FUNCTION pg_temp.b() RETURNS numeric LANGUAGE sql AS $$
  SELECT trending_boost FROM entity_ranking_config WHERE id $$;
CREATE OR REPLACE FUNCTION pg_temp.f(days integer) RETURNS numeric LANGUAGE sql AS $$
  SELECT power(trending_day_factor, days) FROM entity_ranking_config WHERE id $$;
-- The position term at `place`, `days` after the listing, on the global agenda or not.
CREATE OR REPLACE FUNCTION pg_temp.p(place integer, days integer DEFAULT 0, global boolean DEFAULT false)
RETURNS numeric LANGUAGE sql AS $$
  SELECT CASE WHEN place > trending_position_slots THEN 0
              ELSE (CASE WHEN global THEN trending_position_global_base ELSE 0 END + trending_position_base
                    + trending_position_step * (trending_position_slots - place + 1))
                   * power(trending_position_day_factor, days) END
  FROM entity_ranking_config WHERE id $$;

-- ---------------------------------------------------------------------------
-- Fixtures. Days are New York calendar days relative to today.
--   G       the global agenda space (set in the config above); S1, S2 ordinary DAO spaces.
--   UNL     an unlisted claim; DA DB DC DD DE DF, 30 days old, and ORG, created today with 40 votes, are
--           debates on it.
--   AG      today's agenda in G: DC first, DA second.
--   A1      today's agenda in S1: DA first, DB second.
--   A2      today's agenda in S2: DD first.
--   AGY     yesterday's agenda in G: DE first.
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE ids AS
SELECT k, ('b1230000-0000-4000-8000-' || lpad(to_hex(n), 12, '0'))::uuid AS id
FROM unnest(ARRAY['UNL','DA','DB','DC','DD','DE','DF','ORG','AG','A1','A2','AGY','G','S1','S2'])
     WITH ORDINALITY AS u(k, n)
UNION ALL SELECT * FROM (VALUES
  ('DEBATE', 'fd51f935-2063-4617-be39-7b672b23364c'::uuid),
  ('CLAIM',  '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid),
  ('AGENDA', '79e3757b-2630-492c-9d3b-d73a78d45893'::uuid)) AS v(k, id);
SELECT assert((SELECT trending_global_agenda_space_id FROM entity_ranking_config WHERE id) = pg_temp_i_check.id,
              'fixture: G is the configured global space')
FROM (SELECT id FROM ids WHERE k = 'G') pg_temp_i_check;
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


INSERT INTO spaces (id, type, address)
SELECT id, 'DAO'::"spaceTypes", 'addr-' || k FROM ids WHERE k IN ('G','S1','S2');

SELECT pg_temp.ent('UNL', pg_temp.noon(40)), pg_temp.typed('UNL', 'CLAIM');
SELECT pg_temp.debate(k, 'UNL', 30) FROM ids WHERE k IN ('DA','DB','DC','DD','DE','DF');
SELECT pg_temp.debate('ORG', 'UNL', 0), pg_temp.votes('ORG', 40);

SELECT pg_temp.agenda('AG', 'G', 0, ARRAY[]::text[]);
SELECT pg_temp.agenda('A1', 'S1', 0, ARRAY[]::text[]);
SELECT pg_temp.agenda('A2', 'S2', 0, ARRAY[]::text[]);
SELECT pg_temp.agenda('AGY', 'G', 1, ARRAY[]::text[]);
SELECT pg_temp.names('AG', 'DC', 'G'), pg_temp.place('AG', 'DC', 'a0'),
       pg_temp.names('AG', 'DA', 'G'), pg_temp.place('AG', 'DA', 'a1');
SELECT pg_temp.names('A1', 'DA', 'S1'), pg_temp.place('A1', 'DA', 'a0'),
       pg_temp.names('A1', 'DB', 'S1'), pg_temp.place('A1', 'DB', 'a1');
SELECT pg_temp.names('A2', 'DD', 'S2'), pg_temp.place('A2', 'DD', 'a0');
SELECT pg_temp.names('AGY', 'DE', 'G'), pg_temp.place('AGY', 'DE', 'a0');

SELECT trending.refresh_agenda_listings(false);
SELECT public.refresh_entity_ranking_scores(ARRAY[pg_temp.i('ORG'), pg_temp.i('DF')]);

-- ---------------------------------------------------------------------------
-- 1. The tier.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.trend('DC') = pg_temp.b() + pg_temp.p(1, 0, true)
              AND pg_temp.trend('DD') = pg_temp.b() + pg_temp.p(1) AND pg_temp.trend('DB') = pg_temp.b() + pg_temp.p(2),
              '1: a global place adds the global base; space places keep 0122''s term');
SELECT assert(pg_temp.score('DC') > pg_temp.score('DA')
              AND pg_temp.score('DA') > GREATEST(pg_temp.score('DD'), pg_temp.score('DB'), pg_temp.score('ORG')),
              '1: so the global agenda''s order leads, and its last place outranks every space agenda''s first');

-- ---------------------------------------------------------------------------
-- 2. Which place; space agendas; the fade.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.trend('DA') = pg_temp.b() + pg_temp.p(2, 0, true),
              '2: placed 2nd globally and 1st in S1 the same day, a debate takes its global place');
SELECT assert(pg_temp.score('DD') > pg_temp.score('DB') AND pg_temp.score('DB') > pg_temp.score('ORG'),
              '2: space agendas still order their debates, above unplaced ones');
SELECT assert(pg_temp.near(pg_temp.trend('DE'), pg_temp.b() * pg_temp.f(1) + pg_temp.p(1, 1, true)),
              '2: yesterday''s global place fades with the term');
SELECT assert(pg_temp.trend('DF') = 0, '2: an unplaced debate gets nothing');

-- ---------------------------------------------------------------------------
-- 3. Off.
-- ---------------------------------------------------------------------------
UPDATE entity_ranking_config SET trending_global_agenda_space_id = NULL WHERE id;
SELECT trending.refresh_agenda_listings(true);
SELECT assert(pg_temp.trend('DC') = pg_temp.b() + pg_temp.p(1) AND pg_temp.trend('DA') = pg_temp.b() + pg_temp.p(1),
              '3: with no global space, every agenda is a space agenda (DA takes its best place, 1st in S1)');
UPDATE entity_ranking_config SET trending_global_agenda_space_id = (SELECT id FROM ids WHERE k = 'G'),
                                 trending_position_global_base = 0 WHERE id;
SELECT trending.refresh_agenda_listings(true);
SELECT assert(pg_temp.trend('DC') = pg_temp.b() + pg_temp.p(1) AND pg_temp.trend('DA') = pg_temp.b() + pg_temp.p(2),
              '3: a global base of 0 leaves 0122''s term (the global place is still preferred that day)');
UPDATE entity_ranking_config SET trending_position_global_base = 10000 WHERE id;

-- ---------------------------------------------------------------------------
-- 4. Moves with only the global base on.
-- ---------------------------------------------------------------------------
UPDATE entity_ranking_config SET trending_position_step = 0, trending_position_base = 0, trending_position_slots = 1 WHERE id;
SELECT trending.refresh_agenda_listings(true);
SELECT assert(pg_temp.trend('DC') = pg_temp.b() + pg_temp.p(1, 0, true) AND pg_temp.trend('DA') = pg_temp.b(),
              '4: with only the global base, it lifts the global places within the slots');
SELECT pg_temp.place('AG', 'DA', '0');
SELECT assert(trending.refresh_agenda_listings(false) = 2 AND pg_temp.trend('DA') = pg_temp.b() + pg_temp.p(1, 0, true)
              AND pg_temp.trend('DC') = pg_temp.b(),
              '4: and a move across the slots re-scores the two debates that crossed');

-- Cleanup for running by hand.
UPDATE entity_ranking_config SET trending_position_step = 100, trending_position_base = 2500, trending_position_slots = 50,
                                 trending_global_agenda_space_id = '9284477c-8fe7-4636-8698-994e2e517b32' WHERE id;
UPDATE trending.refresh_state SET last_full_pass_on = NULL WHERE id;
TRUNCATE entities, values, relations, votes_count, spaces, entity_ranking_scores, entity_type_ranking,
         entity_topic_ranking, trending.agenda_listings, trending.agenda_debate_positions CASCADE;
DROP TABLE ids;
