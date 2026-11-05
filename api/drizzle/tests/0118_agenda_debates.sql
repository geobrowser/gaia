-- Assertions for 0118: an agenda's own Debates relation lifts just the debates it names (GEO-3592).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts. Expected values are
-- written against entity_ranking_config, so retuning the defaults does not need a new suite.
--
--   1. Listings: a Debates target typed Debate, written in the agenda's own DAO space, is an
--      'agenda_debates' row; one written in another space, a non-debate target and an agenda in a
--      personal space are not; a future-dated agenda's is stored and counts for nothing yet.
--   2. A named debate gets the full listing boost and the listing-day age credit, so a 30-day-old
--      debate ranks above every organic debate; the boost decays like a listing's.
--   3. It is not a claim listing: the debate's claim and the claim's other debates get nothing, and
--      neither carries lastListedOn; a claim listing's debates and discount are untouched.
--   4. A debate named and on a listed claim takes the newer listing.
--   5. currentDebateAgendas returns it as a lever after the claims, with its own lastListedOn, never
--      as an agenda item.
--   6. A vote's re-score keeps its terms; the hourly sweep picks up an added or removed Debates link
--      without touching the claim, and the full pass re-scores every listed debate.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

TRUNCATE entities, values, relations, votes_count, user_votes, spaces, entity_ranking_scores,
         entity_type_weights, entity_type_exclusions, entity_type_ranking, entity_topic_ranking,
         entity_feed_blocklist, personalization.feed_item_signals_hourly, trending.agenda_listings
         CASCADE;
UPDATE trending.refresh_state SET last_full_pass_on = NULL WHERE id;

UPDATE entity_ranking_config
   SET participation_weight = 4, participation_cap = 19, comment_weight = 0, comment_cap = 30,
       tau_seconds = 100000, trending_boost = 8, trending_topic_boost = 2.5, trending_day_factor = 0.65,
       trending_debate_factor = 0.75, trending_debate_window_days = 7, trending_horizon_days = 16
 WHERE id;

CREATE OR REPLACE FUNCTION pg_temp.b() RETURNS numeric LANGUAGE sql AS $$
  SELECT trending_boost FROM entity_ranking_config WHERE id $$;
CREATE OR REPLACE FUNCTION pg_temp.f(days integer) RETURNS numeric LANGUAGE sql AS $$
  SELECT power(trending_day_factor, days) FROM entity_ranking_config WHERE id $$;
CREATE OR REPLACE FUNCTION pg_temp.df() RETURNS numeric LANGUAGE sql AS $$
  SELECT trending_debate_factor FROM entity_ranking_config WHERE id $$;
CREATE OR REPLACE FUNCTION pg_temp.tau() RETURNS numeric LANGUAGE sql AS $$
  SELECT tau_seconds FROM entity_ranking_config WHERE id $$;

-- ---------------------------------------------------------------------------
-- Fixtures. Days are New York calendar days relative to today.
--   CL      an unlisted claim with two debates, 30 days old: DN (named on A0 today) and DSIB.
--   D2      a debate on CL named on A2, 2 days ago.
--   LC      a claim listed on A0's block today, with debate DLC (10 days old, outside the discount
--           window) and DW (2 days old, inside it). DLC is also named on A3, 3 days ago.
--   LC5     a claim listed on A5, 5 days ago, with debate DBOTH, which A0 also names today.
--   DX      a debate named by A0's Debates relation written in S2, not A0's space.
--   NOTD    a claim (not a debate) in A0's Debates relation.
--   DP      a debate named by AP, an agenda in the personal space SP.
--   DF      a debate named only by tomorrow's agenda AFUT.
--   ORG1-5  debates on the unlisted claim UNL, created 0..4 days ago: the organic debate feed.
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE ids AS
SELECT k, ('b1180000-0000-4000-8000-' || lpad(to_hex(n), 12, '0'))::uuid AS id
FROM unnest(ARRAY['CL','LC','LC5','UNL','NOTD',
                  'DN','DSIB','D2','DLC','DW','DBOTH','DX','DP','DF',
                  'ORG1','ORG2','ORG3','ORG4','ORG5',
                  'A0','A2','A3','A5','AP','AFUT','S1','S2','SP']) WITH ORDINALITY AS u(k, n)
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

INSERT INTO spaces (id, type, address)
SELECT id, CASE WHEN k = 'SP' THEN 'Personal' ELSE 'DAO' END::"spaceTypes", 'addr-' || k
FROM ids WHERE k IN ('S1','S2','SP');

SELECT pg_temp.ent(k, pg_temp.noon(40)), pg_temp.typed(k, 'CLAIM') FROM ids WHERE k IN ('CL','LC','LC5','UNL','NOTD');

SELECT pg_temp.agenda('A0', 'S1', 0, ARRAY['LC']);
SELECT pg_temp.agenda('A2', 'S1', 2, ARRAY[]::text[]);
SELECT pg_temp.agenda('A3', 'S1', 3, ARRAY[]::text[]);
SELECT pg_temp.agenda('A5', 'S1', 5, ARRAY['LC5']);
SELECT pg_temp.agenda('AP', 'SP', 0, ARRAY[]::text[]);
SELECT pg_temp.agenda('AFUT', 'S1', -1, ARRAY[]::text[]);

SELECT pg_temp.debate('DN', 'CL', 30), pg_temp.debate('DSIB', 'CL', 30), pg_temp.debate('D2', 'CL', 30);
SELECT pg_temp.debate('DLC', 'LC', 10), pg_temp.debate('DW', 'LC', 2), pg_temp.debate('DBOTH', 'LC5', 20);
SELECT pg_temp.debate('DX', 'UNL', 30), pg_temp.debate('DP', 'UNL', 30), pg_temp.debate('DF', 'UNL', 30);
SELECT pg_temp.debate('ORG' || n, 'UNL', n - 1) FROM generate_series(1, 5) n;

SELECT pg_temp.names('A0', 'DN', 'S1'), pg_temp.names('A0', 'DBOTH', 'S1'), pg_temp.names('A0', 'DX', 'S2'),
       pg_temp.names('A0', 'NOTD', 'S1');
SELECT pg_temp.names('A2', 'D2', 'S1'), pg_temp.names('A3', 'DLC', 'S1');
SELECT pg_temp.names('AP', 'DP', 'SP'), pg_temp.names('AFUT', 'DF', 'S1');

CREATE TEMP TABLE first_run AS SELECT trending.refresh_agenda_listings(false) AS n;
-- The sweep scores only what agendas name; score the rest the way a vote would.
SELECT public.refresh_entity_ranking_scores(ARRAY(SELECT id FROM ids WHERE k IN
  ('CL','UNL','NOTD','DSIB','DX','DP','ORG1','ORG2','ORG3','ORG4','ORG5')));

-- ---------------------------------------------------------------------------
-- 1. Listings.
-- ---------------------------------------------------------------------------
SELECT assert((SELECT array_agg(source) FROM trending.agenda_listings WHERE entity_id = pg_temp.i('DN')
               AND agenda_id = pg_temp.i('A0') AND space_id = pg_temp.i('S1') AND listed_on = pg_temp.today())
              = ARRAY['agenda_debates'],
              '1: a debate in the agenda''s own Debates relation is an agenda_debates listing');
SELECT assert(NOT EXISTS (SELECT 1 FROM trending.agenda_listings WHERE entity_id = pg_temp.i('DX')),
              '1: a Debates relation written outside the agenda''s space is not a listing');
SELECT assert(NOT EXISTS (SELECT 1 FROM trending.agenda_listings WHERE entity_id = pg_temp.i('NOTD'))
              AND pg_temp.trend('NOTD') = 0,
              '1: a claim in the Debates relation is not a listing of any kind');
SELECT assert(NOT EXISTS (SELECT 1 FROM trending.agenda_listings WHERE entity_id = pg_temp.i('DP'))
              AND pg_temp.trend('DP') = 0 AND pg_temp.credit('DP') = 0,
              '1: an agenda in a personal space names no debates');
SELECT assert(EXISTS (SELECT 1 FROM trending.agenda_listings WHERE entity_id = pg_temp.i('DF')
                      AND listed_on = pg_temp.today() + 1)
              AND pg_temp.trend('DF') = 0 AND pg_temp.credit('DF') = 0,
              '1: a future-dated agenda''s debate is stored and counts for nothing until its Date');

-- ---------------------------------------------------------------------------
-- 2. A named debate.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.trend('DN') = pg_temp.b() AND pg_temp.src('DN') = 'listing',
              '2: a named debate gets the full listing boost');
SELECT assert(pg_temp.near(pg_temp.credit('DN'), (pg_temp.midnight(0) - pg_temp.noon(30)) / pg_temp.tau()),
              '2: and the age credit to its listing day');
SELECT assert(pg_temp.score('DN') > (SELECT max(pg_temp.score('ORG' || n)) FROM generate_series(1, 5) n),
              '2: so a 30-day-old debate ranks above every organic debate, including today''s');
SELECT assert(pg_temp.near(pg_temp.trend('D2'), pg_temp.b() * pg_temp.f(2)),
              '2: the boost decays by day_factor per day, as a listing''s does');
SELECT assert(pg_temp.listed_on('DN') = pg_temp.today()
              AND (SELECT public.entities_listed_on_agenda_id(e) FROM entities e WHERE e.id = pg_temp.i('DN')) = pg_temp.i('A0'),
              '2: lastListedOn and listedOnAgendaId are the debate''s own listing (the Trending tag reads them)');

-- ---------------------------------------------------------------------------
-- 3. Not a claim listing.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.trend('CL') = 0 AND pg_temp.credit('CL') = 0 AND pg_temp.listed_on('CL') IS NULL,
              '3: the named debate''s claim gets nothing');
SELECT assert(pg_temp.trend('DSIB') = 0 AND pg_temp.credit('DSIB') = 0 AND pg_temp.listed_on('DSIB') IS NULL,
              '3: nor does the claim''s other debate');
SELECT assert(NOT EXISTS (SELECT 1 FROM trending.claim_listings(ARRAY[pg_temp.i('DN'), pg_temp.i('CL')])),
              '3: a debate listing is never a claim listing');
SELECT assert(pg_temp.near(pg_temp.trend('LC'), pg_temp.b() * pg_temp.df()),
              '3: a listed claim''s discount still counts only its debates in the window (DW)');
SELECT assert(pg_temp.trend('DW') = pg_temp.b(),
              '3: and its debates still get its boost');

-- ---------------------------------------------------------------------------
-- 4. Named and on a listed claim: the newer listing.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.trend('DLC') = pg_temp.b() AND pg_temp.listed_on('DLC') = pg_temp.today()
              AND pg_temp.near(pg_temp.credit('DLC'), (pg_temp.midnight(0) - pg_temp.noon(10)) / pg_temp.tau()),
              '4: named 3 days ago, its claim listed today: the claim''s listing');
SELECT assert(pg_temp.trend('DBOTH') = pg_temp.b() AND pg_temp.listed_on('DBOTH') = pg_temp.today()
              AND pg_temp.near(pg_temp.credit('DBOTH'), (pg_temp.midnight(0) - pg_temp.noon(20)) / pg_temp.tau()),
              '4: named today, its claim listed 5 days ago: its own listing');
SELECT assert(pg_temp.near(pg_temp.trend('LC5'), pg_temp.b() * pg_temp.f(5)) AND pg_temp.listed_on('LC5') = pg_temp.today() - 5,
              '4: and its claim keeps its own, older listing');

-- ---------------------------------------------------------------------------
-- 5. currentDebateAgendas.
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE s1_rows AS
SELECT row_number() OVER () AS n, c.* FROM public.current_debate_agendas(ARRAY[pg_temp.i('S1')]) c;
SELECT assert((SELECT array_agg(source || ':' || (SELECT k FROM ids WHERE id = entity_id) ORDER BY n) FROM s1_rows)
              = ARRAY['block_item:LC', 'agenda_debates:DBOTH', 'agenda_debates:DN']
              OR (SELECT array_agg(source || ':' || (SELECT k FROM ids WHERE id = entity_id) ORDER BY n) FROM s1_rows)
              = ARRAY['block_item:LC', 'agenda_debates:DN', 'agenda_debates:DBOTH'],
              '5: the current agenda''s block items, then its debates as levers; nothing else');
SELECT assert(NOT EXISTS (SELECT 1 FROM s1_rows WHERE source = 'block_item' AND entity_id <> pg_temp.i('LC')),
              '5: a named debate is never an agenda item');
SELECT assert((SELECT last_listed_on FROM s1_rows WHERE entity_id = pg_temp.i('DN')) = pg_temp.today()
              AND (SELECT last_listed_on FROM s1_rows WHERE entity_id = pg_temp.i('LC')) = pg_temp.today(),
              '5: debate and claim rows carry their own lastListedOn');

-- ---------------------------------------------------------------------------
-- 6. Re-scores and the sweep.
-- ---------------------------------------------------------------------------
-- First run, every row new. Claims: LC LC5 (2) and their debates DLC DW DBOTH (3); named debates DN
-- D2 DLC DBOTH DF (DLC and DBOTH already counted: 3 more). Not DX, NOTD or DP: no row.
SELECT assert((SELECT n FROM first_run) = 8, '6: the first run re-scores listed claims, their debates and named debates');
SELECT pg_temp.votes('DN', 3);
SELECT public.refresh_entity_ranking_scores(ARRAY[pg_temp.i('DN')]);
SELECT assert(pg_temp.trend('DN') = pg_temp.b()
              AND pg_temp.near(pg_temp.credit('DN'), (pg_temp.midnight(0) - pg_temp.noon(30)) / pg_temp.tau()),
              '6: a vote''s re-score keeps the boost and the age credit');

SELECT pg_temp.unname('A0', 'DN'), pg_temp.names('A0', 'DSIB', 'S1');
CREATE TEMP TABLE second_run AS SELECT trending.refresh_agenda_listings(false) AS n;
SELECT assert((SELECT n FROM second_run) = 2, '6: an hourly run re-scores the two debates whose link changed, not their claim');
SELECT assert(pg_temp.trend('DN') = 0 AND pg_temp.credit('DN') = 0 AND pg_temp.src('DN') IS NULL,
              '6: an unnamed debate loses its terms');
SELECT assert(pg_temp.trend('DSIB') = pg_temp.b() AND pg_temp.credit('DSIB') > 0, '6: a newly named one gets them');
SELECT assert(pg_temp.trend('CL') = 0, '6: the claim is untouched');

-- Full pass. Listed claims LC LC5 and their debates DLC DW DBOTH (5); named debates DSIB D2 DF (3);
-- nothing else stores a term.
SELECT assert(trending.refresh_agenda_listings(true) = 8, '6: the full pass re-scores every listed debate');
SELECT assert(pg_temp.trend('DSIB') = pg_temp.b() AND pg_temp.near(pg_temp.trend('D2'), pg_temp.b() * pg_temp.f(2)),
              '6: and keeps their terms');

-- D2 stops being a debate: A2 no longer names one, so the hourly run drops the row and clears D2.
DELETE FROM relations WHERE from_entity_id = pg_temp.i('D2') AND to_entity_id = pg_temp.i('DEBATE')
  AND type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid;
SELECT assert(trending.refresh_agenda_listings(false) = 1, '6: an hourly run re-scores a named debate that lost its Debate type');
SELECT assert(NOT EXISTS (SELECT 1 FROM trending.agenda_listings WHERE entity_id = pg_temp.i('D2'))
              AND pg_temp.trend('D2') = 0 AND pg_temp.credit('D2') = 0,
              '6: its listing goes and its terms are cleared, without waiting for the full pass');

-- Cleanup for running by hand.
UPDATE trending.refresh_state SET last_full_pass_on = NULL WHERE id;
TRUNCATE entities, values, relations, votes_count, spaces, entity_ranking_scores, entity_type_ranking,
         entity_topic_ranking, trending.agenda_listings CASCADE;
DROP TABLE ids, first_run, second_run, s1_rows;
