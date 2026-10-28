-- Assertions for 0113: Best ranks what Debate agendas name (GEO-3488).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts. Expected values are
-- written against entity_ranking_config, so retuning the defaults does not need a new suite.
--
--   1. Listings: block items and the agenda's own Claims relation, written in the agenda's own space,
--      claims only; a future-dated agenda is stored but does not count until its Date.
--   2. A claim listed today: age credit to today's midnight plus the full boost; a year-old listed
--      claim outranks a brand-new unlisted one.
--   3. Listed today with no votes beats an unlisted same-day claim with a few votes, not one with
--      very heavy voting.
--   4. Decay: by day_factor per day, newest listing counts (across spaces), zero past the horizon.
--   5. Debates in the past week take debate_factor off each; older debates and non-debates do not.
--   6. Every debate on a listed claim gets the boost, including debates recorded before the listing.
--   7. Debates get the listing-day age credit, only when the listing is newer than the debate: a
--      30-day-old debate on a claim listed today ranks at the top of debates.
--   8. Topics: the agenda's own Topics relation lifts tagged claims and debates by topic_boost, no age
--      credit; the larger of listing and topic applies; a block's Topics and an agenda Topics written
--      in another space do nothing; decay and horizon.
--   9. A vote's re-score keeps the age credit, the listing boost and the topic boost.
--  10. refresh_agenda_listings: picks up new agenda rows, clears removed ones (including a debate's
--      credit), re-scores only what can have moved.
--  11. Config changes clear stored terms; each boost at 0 turns it off; the API fields read the inputs.

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
         trending.agenda_topics CASCADE;

-- The migration's defaults, set explicitly so the suite runs the same after any retune by hand.
UPDATE entity_ranking_config
   SET participation_weight = 4, participation_cap = 19, comment_weight = 0, comment_cap = 30,
       tau_seconds = 100000, trending_boost = 8, trending_topic_boost = 2.5, trending_day_factor = 0.65,
       trending_debate_factor = 0.75, trending_debate_window_days = 7, trending_horizon_days = 16,
       trending_time_zone = 'America/New_York'
 WHERE id;

CREATE OR REPLACE FUNCTION pg_temp.b() RETURNS numeric LANGUAGE sql AS $$
  SELECT trending_boost FROM entity_ranking_config WHERE id $$;
CREATE OR REPLACE FUNCTION pg_temp.tb() RETURNS numeric LANGUAGE sql AS $$
  SELECT trending_topic_boost FROM entity_ranking_config WHERE id $$;
CREATE OR REPLACE FUNCTION pg_temp.f(days integer) RETURNS numeric LANGUAGE sql AS $$
  SELECT power(trending_day_factor, days) FROM entity_ranking_config WHERE id $$;
CREATE OR REPLACE FUNCTION pg_temp.df() RETURNS numeric LANGUAGE sql AS $$
  SELECT trending_debate_factor FROM entity_ranking_config WHERE id $$;
CREATE OR REPLACE FUNCTION pg_temp.tau() RETURNS numeric LANGUAGE sql AS $$
  SELECT tau_seconds FROM entity_ranking_config WHERE id $$;
CREATE OR REPLACE FUNCTION pg_temp.h() RETURNS integer LANGUAGE sql AS $$
  SELECT trending_horizon_days FROM entity_ranking_config WHERE id $$;

-- ---------------------------------------------------------------------------
-- Fixtures. Days are New York calendar days relative to today.
--   OLD     claim created a year ago, listed today.
--   NEW     unlisted claim created now.  FEW: NEW with a few votes (below).  HEAVY: NEW with 200.
--   TWO     listed 2 days ago.   HZN  listed horizon days ago.   PASTHZN  one day past it.
--   MULTI   listed 3 days ago in S1 and 1 day ago in S2.
--   DEB     listed today; debates D1 (today), D2 (2 days ago), D3 (8 days ago).
--   SPAN    listed 5 days ago and again 1 day ago; debate DS created 3 days ago.
--   LATE    listed 2 days ago; debate DL created yesterday (after), DE 4 days ago (before).
--   CDEB    claim created 60 days ago, debated 30 days ago (DOLD), first listed today on AOLD.
--   ORG1-10 debates on an unlisted claim UNL, created 0..9 days ago: the organic debate feed.
--   CDIR    listed only through agenda A0's own Claims relation.
--   CDIRX   named by A0's Claims relation written in S2, not A0's space.
--   FUT     listed only on tomorrow's agenda.   OTHER  Collection item written in S2.
--   PAGE    a non-claim item.   STORY  a news story with a Claims relation to DEB.
-- Topics, named in an agenda's own Topics relation:
--   TOP1  by A0 (today).   TOP2  by A2 (2 days ago).   TOPPAST  by APAST (one day past the horizon).
--   TOPX  by A0, but the Topics relation is written in S2.   TOPB  on A0's block only.
--   TC claim and TD debate (on UNL) tagged TOP1 (TC's tag written in S2); T2 tagged TOP2;
--   TPAST tagged TOPPAST; TX tagged TOPX; TBK tagged TOPB; TF tagged TOPF, named by tomorrow's agenda.
--   BOTHL listed today and tagged TOP1.   BOTHT listed 5 days ago and tagged TOP1.
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE ids AS
SELECT k, ('b1130000-0000-4000-8000-' || lpad(to_hex(n), 12, '0'))::uuid AS id
FROM unnest(ARRAY['OLD','NEW','FEW','HEAVY','TWO','HZN','PASTHZN','MULTI','DEB','SPAN','LATE','CDEB',
                  'UNL','CDIR','CDIRX','FUT','OTHER','PAGE','STORY','TC','T2','TPAST','TX','TBK','TF','BOTHL','BOTHT',
                  'D1','D2','D3','DS','DL','DE','DOLD','TD',
                  'ORG1','ORG2','ORG3','ORG4','ORG5','ORG6','ORG7','ORG8','ORG9','ORG10',
                  'A0','A1','A1S2','A2','A3','A5','AHZN','APAST','AFUT','AOLD',
                  'TOP1','TOP2','TOPPAST','TOPX','TOPB','TOPF','S1','S2']) WITH ORDINALITY AS u(k, n)
UNION ALL SELECT * FROM (VALUES
  ('DEBATE', 'fd51f935-2063-4617-be39-7b672b23364c'::uuid),
  ('CLAIM',  '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid),
  ('AGENDA', '79e3757b-2630-492c-9d3b-d73a78d45893'::uuid),
  ('PAGET',  '480e3fc2-67f3-4993-85fb-cdf4ddeaa6b0'::uuid)) AS v(k, id);
CREATE OR REPLACE FUNCTION pg_temp.i(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM ids WHERE ids.k = $1 $$;
CREATE OR REPLACE FUNCTION pg_temp.today() RETURNS date LANGUAGE sql AS $$
  SELECT (now() AT TIME ZONE 'America/New_York')::date $$;
-- Epoch of noon / midnight New York, `d` days ago.
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
CREATE OR REPLACE FUNCTION pg_temp.tag(k text, topic_k text, space_k text) RETURNS void LANGUAGE sql AS $$
  SELECT pg_temp.rel(k, '806d52bc-27e9-4c91-93c0-57978b093351'::uuid, topic_k, space_k) $$;
-- An agenda in `space_k` dated `d` days ago (negative: the future), one block listing `claims`.
CREATE OR REPLACE FUNCTION pg_temp.agenda(k text, space_k text, d integer, claims text[]) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE block uuid := gen_random_uuid(); c text;
BEGIN
  INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
  VALUES (pg_temp.i(k), '1790000000', '0', '1790000000', '0') ON CONFLICT DO NOTHING;
  PERFORM pg_temp.rel(k, '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid, 'AGENDA', space_k);
  PERFORM pg_temp.rel(k, '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid, 'PAGET', space_k);
  INSERT INTO values (id, entity_id, property_id, space_id, datetime, datetime_utc)
  VALUES (gen_random_uuid()::text, pg_temp.i(k), '52665f3e-fb7d-48d5-8b6b-abb21b0d36db'::uuid, pg_temp.i(space_k),
          'x', to_timestamp(pg_temp.midnight(d)));
  INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
  VALUES (gen_random_uuid(), gen_random_uuid(), 'beaba5cb-a677-41a8-b353-77030613fc70'::uuid, pg_temp.i(k), block, pg_temp.i(space_k), false);
  IF k = 'A0' THEN  -- a block's own Topics relation labels the block and boosts nothing
    INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
    VALUES (gen_random_uuid(), gen_random_uuid(), '806d52bc-27e9-4c91-93c0-57978b093351'::uuid, block, pg_temp.i('TOPB'), pg_temp.i(space_k), false);
  END IF;
  FOREACH c IN ARRAY claims LOOP
    INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
    VALUES (gen_random_uuid(), gen_random_uuid(), 'a99f9ce1-2ffa-4dac-8c61-f6310d46064a'::uuid, block, pg_temp.i(c),
            pg_temp.i(CASE WHEN c = 'OTHER' THEN 'S2' ELSE space_k END), false);
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
  SELECT trending_score FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k) $$;
CREATE OR REPLACE FUNCTION pg_temp.credit(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT age_credit FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k) $$;
CREATE OR REPLACE FUNCTION pg_temp.src(k text) RETURNS text LANGUAGE sql AS $$
  SELECT trending_source FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k) $$;
-- The score without this migration's terms.
CREATE OR REPLACE FUNCTION pg_temp.base(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT ranking_score - age_credit - trending_score FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k) $$;
CREATE OR REPLACE FUNCTION pg_temp.near(a numeric, b numeric) RETURNS boolean LANGUAGE sql AS $$
  SELECT abs(a - b) < 1e-9 $$;

-- The most votes an unlisted claim created now can have and still rank below a claim listed today:
-- 4 ln(1 + v) plus at most one day of recency must stay under the boost.
CREATE TEMP TABLE few AS
SELECT floor(exp((pg_temp.b() - 86400 / pg_temp.tau()) / 4) - 1)::integer AS v;

SELECT pg_temp.ent(k, CASE WHEN k = 'OLD' THEN extract(epoch FROM now())::bigint - 365 * 86400
                           WHEN k IN ('NEW','FEW','HEAVY') THEN extract(epoch FROM now())::bigint
                           WHEN k = 'CDEB' THEN pg_temp.noon(60)
                           ELSE pg_temp.noon(40) END)
FROM ids WHERE k IN ('OLD','NEW','FEW','HEAVY','TWO','HZN','PASTHZN','MULTI','DEB','SPAN','LATE','CDEB',
                     'UNL','CDIR','CDIRX','FUT','OTHER','PAGE','STORY','TC','T2','TPAST','TX','TBK','TF','BOTHL','BOTHT');
SELECT pg_temp.typed(k, 'CLAIM') FROM ids
WHERE k IN ('OLD','NEW','FEW','HEAVY','TWO','HZN','PASTHZN','MULTI','DEB','SPAN','LATE','CDEB',
            'UNL','CDIR','CDIRX','FUT','OTHER','TC','T2','TPAST','TX','TBK','TF','BOTHL','BOTHT');
SELECT pg_temp.typed('PAGE', 'PAGET');
SELECT pg_temp.votes('FEW', (SELECT v FROM few)), pg_temp.votes('HEAVY', 200);

SELECT pg_temp.agenda('A0', 'S1', 0, ARRAY['OLD','DEB','OTHER','PAGE','BOTHL']);
SELECT pg_temp.agenda('AOLD', 'S1', 0, ARRAY['CDEB']);
SELECT pg_temp.agenda('A1', 'S1', 1, ARRAY['SPAN']);
SELECT pg_temp.agenda('A1S2', 'S2', 1, ARRAY['MULTI']);
SELECT pg_temp.agenda('A2', 'S1', 2, ARRAY['TWO','LATE']);
SELECT pg_temp.agenda('A3', 'S1', 3, ARRAY['MULTI']);
SELECT pg_temp.agenda('A5', 'S1', 5, ARRAY['SPAN','BOTHT']);
SELECT pg_temp.agenda('AHZN', 'S1', pg_temp.h(), ARRAY['HZN']);
SELECT pg_temp.agenda('APAST', 'S1', pg_temp.h() + 1, ARRAY['PASTHZN']);
SELECT pg_temp.agenda('AFUT', 'S1', -1, ARRAY['FUT']);
-- The agenda's own Claims and Topics relations.
SELECT pg_temp.rel('A0', 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid, 'CDIR', 'S1');
SELECT pg_temp.rel('A0', 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid, 'CDIRX', 'S2');
SELECT pg_temp.tag('A0', 'TOP1', 'S1'), pg_temp.tag('A2', 'TOP2', 'S1'), pg_temp.tag('APAST', 'TOPPAST', 'S1');
SELECT pg_temp.tag('A0', 'TOPX', 'S2'), pg_temp.tag('AFUT', 'TOPF', 'S1');

SELECT pg_temp.debate('D1', 'DEB', 0), pg_temp.debate('D2', 'DEB', 2), pg_temp.debate('D3', 'DEB', 8);
SELECT pg_temp.debate('DS', 'SPAN', 3);
SELECT pg_temp.debate('DL', 'LATE', 1), pg_temp.debate('DE', 'LATE', 4);
SELECT pg_temp.debate('DOLD', 'CDEB', 30);
SELECT pg_temp.debate('TD', 'UNL', 3);
SELECT pg_temp.debate('ORG' || n, 'UNL', n - 1) FROM generate_series(1, 10) n;
SELECT pg_temp.rel('STORY', 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid, 'DEB', 'S1');

SELECT pg_temp.tag('TC', 'TOP1', 'S2'), pg_temp.tag('TD', 'TOP1', 'S1'), pg_temp.tag('T2', 'TOP2', 'S1'),
       pg_temp.tag('TPAST', 'TOPPAST', 'S1'), pg_temp.tag('TX', 'TOPX', 'S1'), pg_temp.tag('TBK', 'TOPB', 'S1'),
       pg_temp.tag('BOTHL', 'TOP1', 'S1'), pg_temp.tag('BOTHT', 'TOP1', 'S1'), pg_temp.tag('TF', 'TOPF', 'S1');

CREATE TEMP TABLE first_run AS SELECT trending.refresh_agenda_listings() AS n;
-- The sweep scores only what agendas name; score the rest the way a vote would.
SELECT public.refresh_entity_ranking_scores(ARRAY(SELECT id FROM ids WHERE k IN
  ('NEW','FEW','HEAVY','PAGE','STORY','OTHER','UNL','CDIRX','TX','TBK',
   'ORG1','ORG2','ORG3','ORG4','ORG5','ORG6','ORG7','ORG8','ORG9','ORG10')));

-- ---------------------------------------------------------------------------
-- 1. Listings.
-- ---------------------------------------------------------------------------
SELECT assert((SELECT count(*) FROM trending.agenda_listings WHERE claim_id = pg_temp.i('OLD')
               AND agenda_id = pg_temp.i('A0') AND space_id = pg_temp.i('S1') AND listed_on = pg_temp.today()) = 1,
              '1: a claim on today''s agenda is listed today');
SELECT assert(EXISTS (SELECT 1 FROM trending.agenda_listings WHERE claim_id = pg_temp.i('CDIR')
                      AND agenda_id = pg_temp.i('A0')),
              '1: so is a claim in the agenda''s own Claims relation');
SELECT assert(NOT EXISTS (SELECT 1 FROM trending.agenda_listings WHERE claim_id IN (pg_temp.i('OTHER'), pg_temp.i('CDIRX'))),
              '1: a Collection item or Claims relation written outside the agenda''s space is not a listing');
SELECT assert(NOT EXISTS (SELECT 1 FROM trending.agenda_listings WHERE claim_id = pg_temp.i('PAGE')),
              '1: a non-claim item is not a listing');
SELECT assert(EXISTS (SELECT 1 FROM trending.agenda_listings WHERE claim_id = pg_temp.i('FUT')
                      AND listed_on = pg_temp.today() + 1),
              '1: a future-dated agenda is stored');
SELECT assert(pg_temp.credit('FUT') = 0 AND pg_temp.trend('FUT') = 0,
              '1: but counts for nothing until its Date');

-- ---------------------------------------------------------------------------
-- 2. Listed today.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.near(pg_temp.credit('OLD'),
                           (pg_temp.midnight(0) - (SELECT created_at::bigint FROM entities WHERE id = pg_temp.i('OLD'))) / pg_temp.tau()),
              '2: the age credit moves the claim to today''s midnight');
SELECT assert(pg_temp.trend('OLD') = pg_temp.b() AND pg_temp.src('OLD') = 'listing', '2: and adds the full boost');
SELECT assert(pg_temp.near(pg_temp.score('OLD'), pg_temp.base('OLD') + pg_temp.credit('OLD') + pg_temp.b()),
              '2: both are in the ranking score');
SELECT assert(pg_temp.trend('CDIR') = pg_temp.b() AND pg_temp.src('CDIR') = 'listing'
              AND pg_temp.near(pg_temp.credit('CDIR'), (pg_temp.midnight(0) - pg_temp.noon(40)) / pg_temp.tau()),
              '2: a claim in the agenda''s own Claims relation ranks exactly like a block listing');
SELECT assert(pg_temp.score('OLD') > pg_temp.score('NEW'),
              '2: a year-old claim listed today outranks a brand-new unlisted one');
SELECT assert(EXISTS (SELECT 1 FROM entity_type_ranking WHERE type_id = pg_temp.i('CLAIM')
                      AND entity_id = pg_temp.i('OLD') AND ranking_score = pg_temp.score('OLD')),
              '2: Claim''s type ranking (Explore''s claim slot) carries it');

-- ---------------------------------------------------------------------------
-- 3. Against voting.
-- ---------------------------------------------------------------------------
SELECT assert((SELECT v FROM few) >= 1, '3: the boost is worth at least one vote at these settings');
SELECT assert(pg_temp.score('OLD') > pg_temp.score('FEW'),
              '3: listed with no votes beats an unlisted same-day claim with as many votes as the boost is worth');
SELECT assert(pg_temp.score('HEAVY') > pg_temp.score('OLD'),
              '3: but not one with very heavy voting (200)');

-- ---------------------------------------------------------------------------
-- 4. Decay.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.near(pg_temp.trend('TWO'), pg_temp.b() * pg_temp.f(2)), '4: decays by day_factor per day');
SELECT assert(pg_temp.near(pg_temp.trend('HZN'), pg_temp.b() * pg_temp.f(pg_temp.h())), '4: still decaying at the horizon');
SELECT assert(pg_temp.trend('PASTHZN') = 0, '4: and exactly 0 past it');
SELECT assert(pg_temp.credit('PASTHZN') > 0, '4: the age credit has no horizon');
SELECT assert(pg_temp.near(pg_temp.trend('MULTI'), pg_temp.b() * pg_temp.f(1)),
              '4: on agendas in two spaces, the newest listing counts');
SELECT assert(pg_temp.near(pg_temp.credit('MULTI'), (pg_temp.midnight(1) - pg_temp.noon(40)) / pg_temp.tau()),
              '4: for the age credit too');

-- ---------------------------------------------------------------------------
-- 5. Debates in the past week.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.near(pg_temp.trend('DEB'), pg_temp.b() * pg_temp.df() * pg_temp.df()),
              '5: two debates this week take debate_factor off each; the 8-day-old one and the story do not');

-- ---------------------------------------------------------------------------
-- 6. Debates on listed claims: all of them, before or after the listing.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.trend('D1') = pg_temp.b() AND pg_temp.trend('D2') = pg_temp.b() AND pg_temp.trend('D3') = pg_temp.b(),
              '6: every debate on a claim listed today gets its full boost, with no debate discount');
SELECT assert(pg_temp.near(pg_temp.trend('DL'), pg_temp.b() * pg_temp.f(2))
              AND pg_temp.near(pg_temp.trend('DE'), pg_temp.b() * pg_temp.f(2)),
              '6: debates recorded after and before the listing get the same boost, decaying with it');
SELECT assert(pg_temp.near(pg_temp.trend('DS'), pg_temp.b() * pg_temp.f(1)),
              '6: the claim''s newest listing counts');
SELECT assert(COALESCE(pg_temp.trend('STORY'), 0) = 0, '6: a news story with a Claims relation is not a debate');

-- ---------------------------------------------------------------------------
-- 7. The debate age credit.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.near(pg_temp.credit('DOLD'), (pg_temp.midnight(0) - pg_temp.noon(30)) / pg_temp.tau()),
              '7: a 30-day-old debate on a claim first listed today ranks as if published today');
SELECT assert(pg_temp.score('DOLD') > (SELECT max(pg_temp.score('ORG' || n)) FROM generate_series(1, 10) n),
              '7: which puts it above every organic debate, including today''s');
SELECT assert(pg_temp.score('DOLD') - pg_temp.credit('DOLD') < (SELECT min(pg_temp.score('ORG' || n)) FROM generate_series(1, 10) n),
              '7: where the boost alone would leave it below all of them');
SELECT assert(pg_temp.credit('DL') = 0, '7: a debate recorded after the listing keeps its real age');
SELECT assert(pg_temp.near(pg_temp.credit('DE'), (pg_temp.midnight(2) - pg_temp.noon(4)) / pg_temp.tau()),
              '7: one recorded before it moves to the listing day');

-- ---------------------------------------------------------------------------
-- 8. Topics.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.trend('TC') = pg_temp.tb() AND pg_temp.credit('TC') = 0 AND pg_temp.src('TC') = 'topic'
              AND (SELECT trending_topic_id FROM entity_ranking_scores WHERE entity_id = pg_temp.i('TC')) = pg_temp.i('TOP1'),
              '8: a claim tagged (in another space) with an agenda-named topic gets the topic boost, no age credit');
SELECT assert(pg_temp.trend('TD') = pg_temp.tb() AND pg_temp.credit('TD') = 0 AND pg_temp.src('TD') = 'topic',
              '8: so does a debate');
SELECT assert(pg_temp.near(pg_temp.trend('T2'), pg_temp.tb() * pg_temp.f(2)), '8: the topic boost decays');
SELECT assert(pg_temp.trend('TPAST') = 0, '8: and stops past the horizon');
SELECT assert(pg_temp.trend('TF') = 0 AND pg_temp.src('TF') IS NULL,
              '8: a topic on a future-dated agenda counts for nothing until its Date');
SELECT assert(COALESCE(pg_temp.trend('TX'), 0) = 0, '8: an agenda Topics relation written in another space is ignored');
SELECT assert(COALESCE(pg_temp.trend('TBK'), 0) = 0, '8: a block''s Topics relation boosts nothing');
SELECT assert(pg_temp.trend('BOTHL') = GREATEST(pg_temp.b(), pg_temp.tb()) AND pg_temp.src('BOTHL') = 'listing'
              AND (SELECT trending_topic_id FROM entity_ranking_scores WHERE entity_id = pg_temp.i('BOTHL')) IS NULL,
              '8: listed and topic-boosted: the larger (the listing), not both');
SELECT assert(pg_temp.trend('BOTHT') = GREATEST(pg_temp.b() * pg_temp.f(5), pg_temp.tb()) AND pg_temp.src('BOTHT') = 'topic',
              '8: when the listing has decayed below the topic, the topic');
SELECT assert(pg_temp.credit('BOTHT') > 0, '8: and the listing''s age credit stays');

-- ---------------------------------------------------------------------------
-- 9. A vote's re-score keeps every term.
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE before_vote AS SELECT pg_temp.credit('OLD') AS old_credit, pg_temp.credit('DOLD') AS dold_credit;
SELECT pg_temp.votes('OLD', 3), pg_temp.votes('DOLD', 3), pg_temp.votes('TC', 3);
SELECT public.refresh_entity_ranking_scores(ARRAY[pg_temp.i('OLD'), pg_temp.i('DOLD'), pg_temp.i('TC')]);
SELECT assert(pg_temp.credit('OLD') = (SELECT old_credit FROM before_vote) AND pg_temp.trend('OLD') = pg_temp.b(),
              '9: a plain re-score keeps a claim''s age credit and boost');
SELECT assert(pg_temp.credit('DOLD') = (SELECT dold_credit FROM before_vote), '9: and a debate''s age credit');
SELECT assert(pg_temp.trend('TC') = pg_temp.tb(), '9: and the topic boost');

-- ---------------------------------------------------------------------------
-- 10. The hourly rebuild.
-- ---------------------------------------------------------------------------
-- The first run finds every row new. Listed claims: OLD DEB BOTHL CDEB CDIR SPAN MULTI TWO LATE BOTHT
-- HZN PASTHZN FUT (13); their debates D1 D2 D3 DS DL DE DOLD (7); tagged with TOP1 TOP2 TOPPAST:
-- TC TD T2 TPAST TF, BOTHL and BOTHT already counted (5). Never a row: TOPX (A0 is not an agenda in
-- S2) and TOPB (on a block).
SELECT assert((SELECT n FROM first_run) = 25, '10: the first run re-scores everything agendas name');
SELECT pg_temp.rel('A0', 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid, 'NEW', 'S1');
DELETE FROM relations WHERE from_entity_id IN (pg_temp.i('A2'), pg_temp.i('AOLD'))
  AND type_id = 'beaba5cb-a677-41a8-b353-77030613fc70'::uuid;
DELETE FROM relations WHERE from_entity_id = pg_temp.i('A2')
  AND type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid;
CREATE TEMP TABLE second_run AS SELECT trending.refresh_agenda_listings() AS n;
SELECT assert(pg_temp.trend('NEW') = pg_temp.b(), '10: a claim added to an agenda picks up the boost on the next run');
SELECT assert(pg_temp.credit('TWO') = 0 AND pg_temp.trend('TWO') = 0 AND pg_temp.trend('DL') = 0,
              '10: a removed listing clears the claim''s terms and its debates''');
SELECT assert(pg_temp.credit('DOLD') = 0 AND pg_temp.trend('DOLD') = 0,
              '10: removing the agenda clears an old debate''s age credit');
SELECT assert(pg_temp.trend('T2') = 0 AND pg_temp.src('T2') IS NULL, '10: a removed topic clears its boost');
-- Changed: NEW (added), TWO LATE CDEB (removed), TOP2 (removed). Inside the horizon: OLD DEB BOTHL
-- CDIR SPAN MULTI BOTHT HZN, TOP1. Debates: D1 D2 D3 DS DL DE DOLD. Tagged: TC TD T2 (BOTHL BOTHT
-- counted). Not PASTHZN or TPAST (unchanged, past the horizon), FUT or TF (not arrived), TX (other space).
SELECT assert((SELECT n FROM second_run) = 22,
              '10: later runs re-score only what changed or is inside the horizon');

-- ---------------------------------------------------------------------------
-- 11. Config changes, off switches and API fields.
-- ---------------------------------------------------------------------------
SELECT assert(public.entities_last_listed_on(e) = pg_temp.today() - 1
              AND public.entities_listed_on_agenda_id(e) = pg_temp.i('A1'),
              '11: lastListedOn and listedOnAgendaId for a debate are its claim''s')
FROM entities e WHERE e.id = pg_temp.i('DS');
SELECT assert(public.entities_trending_score(e) = pg_temp.b() AND public.entities_trending_age_credit(e) > 0
              AND public.entities_trending_source(e) = 'listing',
              '11: trendingScore, trendingAgeCredit and trendingSource are the stored terms')
FROM entities e WHERE e.id = pg_temp.i('OLD');
SELECT assert(public.entities_trending_source(e) = 'topic' AND public.entities_trending_topic_id(e) = pg_temp.i('TOP1')
              AND public.entities_trending_topic_listed_on(e) = pg_temp.today(),
              '11: trendingTopicId and trendingTopicListedOn name the topic behind a topic boost')
FROM entities e WHERE e.id = pg_temp.i('TC');
SELECT assert(public.entities_last_listed_on(e) IS NULL AND public.entities_trending_source(e) IS NULL,
              '11: nothing for an unlisted, untagged claim')
FROM entities e WHERE e.id = pg_temp.i('FEW');

-- HZN is unchanged and now outside the horizon; only its stored term brings it back to be cleared.
UPDATE entity_ranking_config SET trending_horizon_days = 5 WHERE id;
SELECT trending.refresh_agenda_listings();
SELECT assert(pg_temp.trend('HZN') = 0, '11: a config change clears a stored term the sweep would otherwise miss');
UPDATE entity_ranking_config SET trending_horizon_days = 16 WHERE id;

UPDATE entity_ranking_config SET trending_topic_boost = 0 WHERE id;
SELECT trending.refresh_agenda_listings();
SELECT assert(NOT EXISTS (SELECT 1 FROM entity_ranking_scores WHERE trending_source = 'topic'
                          OR (entity_id IN (pg_temp.i('TC'), pg_temp.i('TD')) AND trending_score <> 0)),
              '11: topic boost 0: no topic term anywhere');
SELECT assert(pg_temp.trend('OLD') = pg_temp.b(), '11: listings untouched');

UPDATE entity_ranking_config SET trending_boost = 0 WHERE id;
SELECT trending.refresh_agenda_listings();
SELECT assert(NOT EXISTS (SELECT 1 FROM entity_ranking_scores WHERE trending_score <> 0),
              '11: both boosts 0: no trending term anywhere');
SELECT assert(pg_temp.credit('OLD') > 0 AND pg_temp.credit('D3') > 0, '11: the age credit stays');

-- Cleanup for running by hand.
UPDATE entity_ranking_config
   SET trending_boost = 8, trending_topic_boost = 2.5, trending_day_factor = 0.65, trending_debate_factor = 0.75,
       trending_debate_window_days = 7, trending_horizon_days = 16, trending_time_zone = 'America/New_York'
 WHERE id;
TRUNCATE entities, values, relations, votes_count, entity_ranking_scores, entity_type_ranking,
         entity_topic_ranking, trending.agenda_listings, trending.agenda_topics CASCADE;
DROP TABLE ids, few, first_run, second_run, before_vote;
