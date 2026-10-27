-- Assertions for 0113: Best ranks agenda claims, then the debates recorded on them (GEO-3488).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
--   1. Listings: Blocks → Collection item, every relation in the agenda's own space, claims only;
--      a future-dated agenda is stored but does not count until its Date.
--   2. A claim listed today: age credit to today's midnight plus the full boost; a year-old listed
--      claim outranks a brand-new unlisted one.
--   3. Listed today with no votes beats an unlisted same-day claim with ordinary voting, not one
--      with very heavy voting.
--   4. Decay: halves per day, newest listing counts (across spaces), zero past the horizon.
--   5. Debates in the past week take a quarter off each; older debates and non-debates do not.
--   6. Debates: same boost as the claim's listing, no debate discount, nothing for a debate created
--      before the claim was first listed; every debate after a listing gets the same boost.
--   7. The age credit survives a plain re-score (what a vote does).
--   8. refresh_agenda_listings: picks up a new agenda, clears a removed one, re-scores only what can
--      have moved.
--   9. Config changes clear stored terms; boost 0 turns the boost off; the API fields read the inputs.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

TRUNCATE entities, values, relations, votes_count, user_votes, spaces, entity_ranking_scores,
         entity_type_weights, entity_type_exclusions, entity_type_ranking, entity_topic_ranking,
         entity_feed_blocklist, personalization.feed_item_signals_hourly, trending.agenda_listings CASCADE;

UPDATE entity_ranking_config
   SET participation_weight = 4, participation_cap = 19, comment_weight = 0, comment_cap = 30,
       tau_seconds = 100000, trending_boost = 15, trending_day_factor = 0.5,
       trending_debate_factor = 0.75, trending_debate_window_days = 7, trending_horizon_days = 11,
       trending_time_zone = 'America/New_York'
 WHERE id;

-- ---------------------------------------------------------------------------
-- Fixtures. Days are New York calendar days relative to today.
--   OLD     claim created a year ago, listed today in S1.
--   NEW     unlisted claim created now.          VOTED   NEW with 20 votes.   HEAVY  NEW with 200.
--   TWO     claim listed 2 days ago.             ELEVEN  listed 11 days ago.  TWELVE listed 12 days ago.
--   MULTI   listed 3 days ago in S1 and 1 day ago in S2.
--   DEB     claim listed 3 days ago and today; debates D1 (today), D2 (2 days ago), D3 (8 days ago).
--   SPAN    listed 5 days ago and again 1 day ago; debate DS created 3 days ago.
--   LATE    listed 2 days ago; debate DL created yesterday, DE created 4 days ago (before listing).
--   FUT     listed only on tomorrow's agenda.
--   OTHER   on an agenda's block, but the Collection item is written in S2, not the agenda's S1.
--   PAGE    a non-claim on the block.
--   STORY   a news story with a Claims relation to DEB (not a debate).
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE ids AS SELECT * FROM (VALUES
  ('OLD',    'b1130000-0000-4000-8000-000000000101'::uuid),
  ('NEW',    'b1130000-0000-4000-8000-000000000102'::uuid),
  ('VOTED',  'b1130000-0000-4000-8000-000000000103'::uuid),
  ('HEAVY',  'b1130000-0000-4000-8000-000000000104'::uuid),
  ('TWO',    'b1130000-0000-4000-8000-000000000105'::uuid),
  ('ELEVEN', 'b1130000-0000-4000-8000-000000000106'::uuid),
  ('TWELVE', 'b1130000-0000-4000-8000-000000000107'::uuid),
  ('MULTI',  'b1130000-0000-4000-8000-000000000108'::uuid),
  ('DEB',    'b1130000-0000-4000-8000-000000000109'::uuid),
  ('SPAN',   'b1130000-0000-4000-8000-00000000010a'::uuid),
  ('LATE',   'b1130000-0000-4000-8000-00000000010b'::uuid),
  ('FUT',    'b1130000-0000-4000-8000-00000000010c'::uuid),
  ('OTHER',  'b1130000-0000-4000-8000-00000000010d'::uuid),
  ('PAGE',   'b1130000-0000-4000-8000-00000000010e'::uuid),
  ('D1',     'b1130000-0000-4000-8000-000000000201'::uuid),
  ('D2',     'b1130000-0000-4000-8000-000000000202'::uuid),
  ('D3',     'b1130000-0000-4000-8000-000000000203'::uuid),
  ('DS',     'b1130000-0000-4000-8000-000000000204'::uuid),
  ('DL',     'b1130000-0000-4000-8000-000000000205'::uuid),
  ('DE',     'b1130000-0000-4000-8000-000000000206'::uuid),
  ('STORY',  'b1130000-0000-4000-8000-000000000301'::uuid),
  ('A0',     'b1130000-0000-4000-8000-000000000401'::uuid),
  ('A1',     'b1130000-0000-4000-8000-000000000402'::uuid),
  ('A2',     'b1130000-0000-4000-8000-000000000403'::uuid),
  ('A3',     'b1130000-0000-4000-8000-000000000404'::uuid),
  ('A5',     'b1130000-0000-4000-8000-000000000405'::uuid),
  ('A11',    'b1130000-0000-4000-8000-000000000406'::uuid),
  ('A12',    'b1130000-0000-4000-8000-000000000407'::uuid),
  ('AFUT',   'b1130000-0000-4000-8000-000000000408'::uuid),
  ('A1S2',   'b1130000-0000-4000-8000-000000000409'::uuid),
  ('S1',     'b1130000-0000-4000-8000-0000000000d1'::uuid),
  ('S2',     'b1130000-0000-4000-8000-0000000000d2'::uuid),
  ('DEBATE', 'fd51f935-2063-4617-be39-7b672b23364c'::uuid),
  ('CLAIM',  '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid),
  ('AGENDA', '79e3757b-2630-492c-9d3b-d73a78d45893'::uuid),
  ('PAGET',  '480e3fc2-67f3-4993-85fb-cdf4ddeaa6b0'::uuid)
) AS v(k, id);
CREATE OR REPLACE FUNCTION pg_temp.i(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM ids WHERE ids.k = $1 $$;
CREATE OR REPLACE FUNCTION pg_temp.today() RETURNS date LANGUAGE sql AS $$
  SELECT (now() AT TIME ZONE 'America/New_York')::date $$;
-- Epoch of noon New York, `d` days ago.
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
-- The score without this migration's terms.
CREATE OR REPLACE FUNCTION pg_temp.base(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT ranking_score - age_credit - trending_score FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k) $$;
CREATE OR REPLACE FUNCTION pg_temp.near(a numeric, b numeric) RETURNS boolean LANGUAGE sql AS $$
  SELECT abs(a - b) < 1e-9 $$;

SELECT pg_temp.ent(k, CASE k WHEN 'OLD' THEN extract(epoch FROM now())::bigint - 365 * 86400
                             WHEN 'NEW' THEN extract(epoch FROM now())::bigint
                             WHEN 'VOTED' THEN extract(epoch FROM now())::bigint
                             WHEN 'HEAVY' THEN extract(epoch FROM now())::bigint
                             ELSE pg_temp.noon(40) END)
FROM ids WHERE k IN ('OLD','NEW','VOTED','HEAVY','TWO','ELEVEN','TWELVE','MULTI','DEB','SPAN','LATE','FUT','OTHER','PAGE','STORY');
SELECT pg_temp.typed(k, 'CLAIM') FROM ids
WHERE k IN ('OLD','NEW','VOTED','HEAVY','TWO','ELEVEN','TWELVE','MULTI','DEB','SPAN','LATE','FUT','OTHER');
SELECT pg_temp.typed('PAGE', 'PAGET');
SELECT pg_temp.votes('VOTED', 20), pg_temp.votes('HEAVY', 200);

SELECT pg_temp.agenda('A0', 'S1', 0, ARRAY['OLD','DEB','OTHER','PAGE']);
SELECT pg_temp.agenda('A1', 'S1', 1, ARRAY['SPAN']);
SELECT pg_temp.agenda('A1S2', 'S2', 1, ARRAY['MULTI']);
SELECT pg_temp.agenda('A2', 'S1', 2, ARRAY['TWO','LATE']);
SELECT pg_temp.agenda('A3', 'S1', 3, ARRAY['MULTI','DEB']);
SELECT pg_temp.agenda('A5', 'S1', 5, ARRAY['SPAN']);
SELECT pg_temp.agenda('A11', 'S1', 11, ARRAY['ELEVEN']);
SELECT pg_temp.agenda('A12', 'S1', 12, ARRAY['TWELVE']);
SELECT pg_temp.agenda('AFUT', 'S1', -1, ARRAY['FUT']);

SELECT pg_temp.debate('D1', 'DEB', 0), pg_temp.debate('D2', 'DEB', 2), pg_temp.debate('D3', 'DEB', 8);
SELECT pg_temp.debate('DS', 'SPAN', 3);
SELECT pg_temp.debate('DL', 'LATE', 1), pg_temp.debate('DE', 'LATE', 4);
SELECT pg_temp.rel('STORY', 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid, 'DEB', 'S1');

CREATE TEMP TABLE first_run AS SELECT trending.refresh_agenda_listings() AS n;
-- The sweep scores only what is listed; score the unlisted fixtures the way a vote would.
SELECT public.refresh_entity_ranking_scores(ARRAY(SELECT id FROM ids WHERE k IN ('NEW','VOTED','HEAVY','PAGE','STORY','FUT','OTHER')));

-- ---------------------------------------------------------------------------
-- 1. Listings.
-- ---------------------------------------------------------------------------
SELECT assert((SELECT count(*) FROM trending.agenda_listings WHERE claim_id = pg_temp.i('OLD')
               AND agenda_id = pg_temp.i('A0') AND space_id = pg_temp.i('S1') AND listed_on = pg_temp.today()) = 1,
              '1: a claim on today''s agenda is listed today');
SELECT assert(NOT EXISTS (SELECT 1 FROM trending.agenda_listings WHERE claim_id = pg_temp.i('OTHER')),
              '1: a Collection item written outside the agenda''s space is not a listing');
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
                           (pg_temp.midnight(0) - (SELECT created_at::bigint FROM entities WHERE id = pg_temp.i('OLD'))) / 100000.0),
              '2: the age credit moves the claim to today''s midnight');
SELECT assert(pg_temp.trend('OLD') = 15, '2: and adds the full boost');
SELECT assert(pg_temp.near(pg_temp.score('OLD'), pg_temp.base('OLD') + pg_temp.credit('OLD') + 15),
              '2: both are in the ranking score');
SELECT assert(pg_temp.score('OLD') > pg_temp.score('NEW'),
              '2: a year-old claim listed today outranks a brand-new unlisted one');
SELECT assert(EXISTS (SELECT 1 FROM entity_type_ranking WHERE type_id = pg_temp.i('CLAIM')
                      AND entity_id = pg_temp.i('OLD') AND ranking_score = pg_temp.score('OLD')),
              '2: Claim''s type ranking (Explore''s claim slot) carries it');

-- ---------------------------------------------------------------------------
-- 3. Against voting.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.score('OLD') > pg_temp.score('VOTED'),
              '3: listed with no votes beats an unlisted same-day claim with 20 votes');
SELECT assert(pg_temp.score('HEAVY') > pg_temp.score('OLD'),
              '3: but not one with very heavy voting (200)');

-- ---------------------------------------------------------------------------
-- 4. Decay.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.near(pg_temp.trend('TWO'), 15 * 0.25), '4: two days on, a quarter of the boost');
SELECT assert(pg_temp.near(pg_temp.trend('ELEVEN'), 15 * power(0.5::numeric, 11)),
              '4: still decaying at the horizon');
SELECT assert(pg_temp.trend('TWELVE') = 0, '4: and exactly 0 past it');
SELECT assert(pg_temp.credit('TWELVE') > 0, '4: the age credit has no horizon');
SELECT assert(pg_temp.near(pg_temp.trend('MULTI'), 15 * 0.5),
              '4: on agendas in two spaces, the newest listing counts');
SELECT assert(pg_temp.near(pg_temp.credit('MULTI'), (pg_temp.midnight(1) - pg_temp.noon(40)) / 100000.0),
              '4: for the age credit too');

-- ---------------------------------------------------------------------------
-- 5. Debates in the past week.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.near(pg_temp.trend('DEB'), 15 * 0.75 * 0.75),
              '5: two debates this week take a quarter off each; the 8-day-old one and the story do not');

-- ---------------------------------------------------------------------------
-- 6. Debates on listed claims.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.trend('D1') = 15 AND pg_temp.trend('D2') = 15,
              '6: every debate since the claim was first listed gets its full boost, with no debate discount');
SELECT assert(pg_temp.trend('D3') = 0, '6: one from before the first listing gets nothing');
SELECT assert(pg_temp.credit('D1') = 0, '6: debates get no age credit');
SELECT assert(pg_temp.near(pg_temp.trend('DL'), 15 * 0.25), '6: a debate decays with its claim''s listing');
SELECT assert(pg_temp.trend('DE') = 0, '6: a debate created before its claim was first listed gets nothing');
SELECT assert(pg_temp.near(pg_temp.trend('DS'), 15 * 0.5),
              '6: a debate between two listings takes the newest listing''s boost');
SELECT assert(COALESCE(pg_temp.trend('STORY'), 0) = 0, '6: a news story with a Claims relation is not a debate');

-- ---------------------------------------------------------------------------
-- 7. A vote's re-score keeps the credit.
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE before_vote AS SELECT pg_temp.credit('OLD') AS credit;
SELECT pg_temp.votes('OLD', 3);
SELECT public.refresh_entity_ranking_scores(ARRAY[pg_temp.i('OLD')]);
SELECT assert(pg_temp.credit('OLD') = (SELECT credit FROM before_vote) AND pg_temp.trend('OLD') = 15,
              '7: a plain re-score keeps the age credit and the boost');

-- ---------------------------------------------------------------------------
-- 8. The hourly rebuild.
-- ---------------------------------------------------------------------------
-- The first run finds every listing new: all nine listed claims (FUT and TWELVE included, so their
-- terms are written as 0) and the six debates on them.
SELECT assert((SELECT n FROM first_run) = 15, '8: the first run re-scores every listed claim and its debates');
SELECT pg_temp.agenda('A0', 'S1', 0, ARRAY['NEW']);
DELETE FROM relations WHERE from_entity_id = pg_temp.i('A2')
  AND type_id = 'beaba5cb-a677-41a8-b353-77030613fc70'::uuid;
CREATE TEMP TABLE second_run AS SELECT trending.refresh_agenda_listings() AS n;
SELECT assert(pg_temp.trend('NEW') = 15, '8: a claim added to an agenda picks up the boost on the next run');
SELECT assert(pg_temp.credit('TWO') = 0 AND pg_temp.trend('TWO') = 0 AND pg_temp.trend('DL') = 0,
              '8: a removed listing clears the claim''s terms and its debates''');
-- Changed: NEW (added), TWO and LATE (removed). Inside the horizon: OLD DEB MULTI SPAN ELEVEN NEW.
-- Their debates: D1 D2 D3 DS DL DE. Not TWELVE (unchanged, past the horizon) or FUT (not arrived).
SELECT assert((SELECT n FROM second_run) = 14,
              '8: later runs re-score only what changed or is inside the horizon, and their debates');

-- ---------------------------------------------------------------------------
-- 9. Off switch and API fields.
-- ---------------------------------------------------------------------------
SELECT assert(public.entities_last_listed_on(e) = pg_temp.today() - 1
              AND public.entities_listed_on_agenda_id(e) = pg_temp.i('A1'),
              '9: lastListedOn and listedOnAgendaId for a debate are its claim''s')
FROM entities e WHERE e.id = pg_temp.i('DS');
SELECT assert(public.entities_trending_score(e) = 15 AND public.entities_trending_age_credit(e) > 0,
              '9: trendingScore and trendingAgeCredit are the stored terms')
FROM entities e WHERE e.id = pg_temp.i('OLD');
SELECT assert(public.entities_last_listed_on(e) IS NULL, '9: nothing for an unlisted claim')
FROM entities e WHERE e.id = pg_temp.i('VOTED');

-- ELEVEN is unchanged and now outside the horizon; only its stored term brings it back to be cleared.
UPDATE entity_ranking_config SET trending_horizon_days = 5 WHERE id;
SELECT trending.refresh_agenda_listings();
SELECT assert(pg_temp.trend('ELEVEN') = 0, '9: a config change clears a stored term the sweep would otherwise miss');
UPDATE entity_ranking_config SET trending_horizon_days = 11 WHERE id;

UPDATE entity_ranking_config SET trending_boost = 0 WHERE id;
SELECT trending.refresh_agenda_listings();
SELECT assert(NOT EXISTS (SELECT 1 FROM entity_ranking_scores WHERE trending_score <> 0),
              '9: boost 0: no trending term anywhere');
SELECT assert(pg_temp.credit('OLD') > 0, '9: the age credit stays');

-- Cleanup for running by hand.
UPDATE entity_ranking_config
   SET trending_boost = 15, trending_day_factor = 0.5, trending_debate_factor = 0.75,
       trending_debate_window_days = 7, trending_horizon_days = 11, trending_time_zone = 'America/New_York'
 WHERE id;
TRUNCATE entities, values, relations, votes_count, entity_ranking_scores, entity_type_ranking,
         entity_topic_ranking, trending.agenda_listings CASCADE;
DROP TABLE ids, first_run, second_run, before_vote;
