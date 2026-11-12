-- Assertions for 0124: the top of Best can follow the Best debates ranking instead of agendas, behind one
-- switch, off by default (GEO-3681).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts. Expected values are written
-- against entity_ranking_config and the sources rows, so retuning them does not need a new suite.
--
--   0. Shipped off: the mode defaults to 0 and the sources table is empty. With sources, ballots, scores and
--      member downvotes all present and the mode at 0, every stored score is exactly what it was before any
--      of them existed, the new tables stay empty and no report is written.
--   1. Shadow (1): places and push-downs are kept and the daily report is written, but every score is still
--      exactly the off one.
--   2. On (2): a ranked debate with no agenda listing gets a place; a debate in both rows keeps its best
--      weighted place; only Debate-typed, non-deleted (present, not blocklisted) debates take places, deduped
--      by entity; New is renumbered among the last 7 days. Agendas stop setting places (their plain boost stays).
--   3. Push-down: members' (members ∪ editors) curation downvotes since the launch time, matched on
--      user_votes.user_id, against members whose latest ballot holds the debate. Pushed down = no place and a
--      flat −100 after the GREATEST; it sits below unranked debates created within 30 days of it. A new member
--      downvote pushes a debate down within the hour.
--   4. Rollback to shadow, then off: a full pass gives back exactly the off scores and empties the tables.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

-- 0. What ships, checked before this suite touches anything.
SELECT assert((SELECT column_default FROM information_schema.columns
               WHERE table_name = 'entity_ranking_config' AND column_name = 'ranking_places_mode') = '0',
              '0: the switch defaults to 0 (off)');
SELECT assert((SELECT column_default FROM information_schema.columns
               WHERE table_name = 'entity_ranking_config' AND column_name = 'ranking_pushdown_since') = '0',
              '0: the push-down launch time defaults to 0 (counts nothing)');
SELECT assert((SELECT column_default FROM information_schema.columns
               WHERE table_name = 'entity_ranking_config' AND column_name = 'ranking_pushdown') = '150',
              '0: the push-down defaults to 150');
-- gaia#1483 (GEO-3631) and any later migration that re-creates these from 0122's bodies would silently undo the
-- cutover: the live definitions must still read the switch.
SELECT assert(pg_get_functiondef('trending.entity_trending_terms(uuid[])'::regprocedure) LIKE '%ranking_places_mode%'
              AND pg_get_functiondef('trending.refresh_agenda_listings(boolean)'::regprocedure) LIKE '%ranking_places_mode%',
              '0: entity_trending_terms and refresh_agenda_listings still read ranking_places_mode');
SELECT assert(pg_get_functiondef('trending.refresh_agenda_listings(boolean)'::regprocedure)
                LIKE '%current_ranking_places(ARRAY(SELECT entity_id FROM incoming_ranking_pushdowns))%',
              '0: the sweep numbers places from the push-downs it just stored (one snapshot of the votes)');
SELECT assert(NOT EXISTS (SELECT 1 FROM trending.ranking_place_sources), '0: the sources table ships empty');

TRUNCATE entities, values, relations, votes_count, user_votes, spaces, entity_ranking_scores,
         entity_type_weights, entity_type_exclusions, entity_type_ranking, entity_topic_ranking,
         entity_feed_blocklist, personalization.feed_item_signals_hourly, trending.agenda_listings,
         trending.agenda_debate_positions, trending.ranking_place_sources, trending.ranking_places,
         trending.ranking_pushdowns, trending.ranking_shadow_report, trending.ranking_shadow_days,
         ranks.rankings, ranks.ranking_items, ranks.ranking_scores, ranks.members, ranks.editors
         CASCADE;
UPDATE trending.refresh_state SET last_full_pass_on = NULL WHERE id;

UPDATE entity_ranking_config
   SET participation_weight = 4, participation_cap = 19, comment_weight = 0, comment_cap = 30,
       tau_seconds = 45000, trending_boost = 8, trending_topic_boost = 2.5, trending_day_factor = 0.65,
       trending_debate_factor = 0.75, trending_debate_window_days = 7, trending_horizon_days = 16,
       trending_position_base = 2500, trending_position_step = 100, trending_position_slots = 50,
       trending_position_day_factor = 0.3,
       ranking_places_mode = 0, ranking_place_base = 2500, ranking_place_step = 100, ranking_pushdown = 150,
       ranking_pushdown_since = 0
 WHERE id;

-- ---------------------------------------------------------------------------
-- Fixtures. Days are New York calendar days before today; debates are created at noon.
--   UNL     an unlisted claim every debate is on.
--   A0      today's agenda in S1, naming D1 at place 1 and LPD (no position).
--   On the Best debates block BLK (space DEB), ranks.ranking_scores positions:
--     1 PD (1 day old)   ranked by M1 only, downvoted by M2 and M3 since launch: pushed down
--     2 R1 (today)       New #1 and All time #1
--     3 R2 (3 days)      New #2 and All time #2: All time's weighted value is the larger
--     4 NOTD             typed Claim, not Debate: no place
--     5 ROLD (10 days)   outside New's 7 days: All time #3 only
--     6 BLK1 (2 days)    blocklisted: no place
--     7 DUP (2 days)     also at 9 under a second item space: one entity, its best position. New #3, All time #4
--     8 BAL (20 days)    ranked by M1, downvoted by M2: a tie is not a push-down. All time #5
--    10 GONE             no entities row: no place
--   Not ranked: LPD (listed by A0, downvoted by M2 and M3: listing − 100); EARLY (downvoted by M2 and M3
--   before launch); NONMEM (downvoted by N1 and N2, not members, one vote cast from DEB); ELSE (downvoted by
--   M3 from another space: counts); SUPER (on M1's superseded ballot, downvoted by M2: pushed down);
--   UNR30 (31 days old, no votes) and UNRNEW (today); D1 (30 days, agenda place 1).
--   Members: M1, M2 members of DEB, M3 an editor; N1, N2 personal spaces with no role. M1 and M2 have ballots.
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE ids AS
SELECT k, ('b1230000-0000-4000-8000-' || lpad(to_hex(n), 12, '0'))::uuid AS id
FROM unnest(ARRAY['UNL','A0','S1','DEB','BLK','M1','M2','M3','N1','N2',
                  'PD','R1','R2','NOTD','ROLD','BLK1','DUP','BAL','GONE','LPD','EARLY','NONMEM','ELSE','SUPER',
                  'UNR30','UNRNEW','D1','DL30','UOLD','BAL1','B1OLD','B1','B2','S2']) WITH ORDINALITY AS u(k, n)
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
CREATE OR REPLACE FUNCTION pg_temp.debate(k text, d integer) RETURNS void LANGUAGE sql AS $$
  SELECT pg_temp.ent(k, pg_temp.noon(d));
  SELECT pg_temp.typed(k, 'DEBATE');
  SELECT pg_temp.rel(k, 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid, 'UNL', 'S1');
$$;
-- A Debate agenda in `space_k` dated `d` days ago, with no claims.
CREATE OR REPLACE FUNCTION pg_temp.agenda(k text, space_k text, d integer) RETURNS void LANGUAGE sql AS $$
  INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
  VALUES (pg_temp.i(k), '1790000000', '0', '1790000000', '0');
  SELECT pg_temp.rel(k, '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid, 'AGENDA', space_k);
  INSERT INTO values (id, entity_id, property_id, space_id, datetime, datetime_utc)
  VALUES (gen_random_uuid()::text, pg_temp.i(k), '52665f3e-fb7d-48d5-8b6b-abb21b0d36db'::uuid, pg_temp.i(space_k),
          'x', to_timestamp(pg_temp.midnight(d)));
$$;
CREATE OR REPLACE FUNCTION pg_temp.names(agenda_k text, debate_k text, space_k text, pos text) RETURNS void LANGUAGE sql AS $$
  INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system, position)
  VALUES (gen_random_uuid(), gen_random_uuid(), 'b96bf701-a399-430d-a072-f5e910cdeda9'::uuid, pg_temp.i(agenda_k),
          pg_temp.i(debate_k), pg_temp.i(space_k), false, pos)
$$;
-- A curation downvote by personal space `who` on `k`, cast from `from_k`, `hours` before now.
CREATE OR REPLACE FUNCTION pg_temp.down(who text, k text, from_k text, hours integer) RETURNS void LANGUAGE sql AS $$
  INSERT INTO user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at)
  VALUES (pg_temp.i(who), pg_temp.i(k), 0, pg_temp.i(from_k), 1, 0, now() - make_interval(hours => hours))
$$;
CREATE OR REPLACE FUNCTION pg_temp.ballot(rk text, who text, blk bigint, items text[]) RETURNS void LANGUAGE sql AS $$
  INSERT INTO ranks.rankings (id, block_id, space_id, updated_at_block, update_index)
  VALUES (pg_temp.i(rk), pg_temp.i('BLK'), pg_temp.i(who), blk, 0);
  INSERT INTO ranks.ranking_items (ranking_id, entity_id, space_id, position)
  SELECT pg_temp.i(rk), pg_temp.i(x), pg_temp.i('S1'), 'a' || o FROM unnest(items) WITH ORDINALITY AS u(x, o);
$$;
CREATE OR REPLACE FUNCTION pg_temp.scored(k text, pos integer, space_k text DEFAULT 'S1') RETURNS void LANGUAGE sql AS $$
  INSERT INTO ranks.ranking_scores (block_id, entity_id, space_id, score, position)
  VALUES (pg_temp.i('BLK'), pg_temp.i(k), pg_temp.i(space_k), 1.0 / pos, pos)
$$;

CREATE OR REPLACE FUNCTION pg_temp.score(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT ranking_score FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k) $$;
CREATE OR REPLACE FUNCTION pg_temp.trend(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT COALESCE((SELECT trending_score FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k)), 0) $$;
CREATE OR REPLACE FUNCTION pg_temp.src(k text) RETURNS text LANGUAGE sql AS $$
  SELECT trending_source FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k) $$;
CREATE OR REPLACE FUNCTION pg_temp.place(src text, k text) RETURNS integer LANGUAGE sql AS $$
  SELECT place FROM trending.ranking_places WHERE source = src AND entity_id = pg_temp.i(k) $$;
CREATE OR REPLACE FUNCTION pg_temp.pushed(k text) RETURNS boolean LANGUAGE sql AS $$
  SELECT EXISTS (SELECT 1 FROM trending.ranking_pushdowns WHERE entity_id = pg_temp.i(k)) $$;
CREATE OR REPLACE FUNCTION pg_temp.near(a numeric, b numeric) RETURNS boolean LANGUAGE sql AS $$
  SELECT abs(a - b) < 1e-9 $$;
-- A source's place value at `place` for a debate `age` days old.
CREATE OR REPLACE FUNCTION pg_temp.pv(src text, place integer, age integer) RETURNS numeric LANGUAGE sql AS $$
  SELECT s.weight * (c.ranking_place_base + c.ranking_place_step * (s.slots - place + 1)) * power(s.day_factor, age)
  FROM trending.ranking_place_sources s CROSS JOIN entity_ranking_config c WHERE c.id AND s.name = src $$;
CREATE OR REPLACE FUNCTION pg_temp.pd() RETURNS numeric LANGUAGE sql AS $$
  SELECT ranking_pushdown FROM entity_ranking_config WHERE id $$;
CREATE OR REPLACE FUNCTION pg_temp.credit(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT COALESCE((SELECT age_credit FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k)), 0) $$;
CREATE OR REPLACE FUNCTION pg_temp.report(k text) RETURNS trending.ranking_shadow_report LANGUAGE sql AS $$
  SELECT * FROM trending.ranking_shadow_report WHERE report_on = pg_temp.today() AND entity_id = pg_temp.i(k) $$;
CREATE OR REPLACE FUNCTION pg_temp.b() RETURNS numeric LANGUAGE sql AS $$
  SELECT trending_boost FROM entity_ranking_config WHERE id $$;
CREATE OR REPLACE FUNCTION pg_temp.p1() RETURNS numeric LANGUAGE sql AS $$
  SELECT trending_position_base + trending_position_step * trending_position_slots FROM entity_ranking_config WHERE id $$;
CREATE OR REPLACE FUNCTION pg_temp.full_pass() RETURNS integer LANGUAGE sql AS $$
  SELECT trending.refresh_agenda_listings(true) $$;
-- Every stored score row, for comparing two states exactly.
CREATE TEMP TABLE snaps (tag text, entity_id uuid, ranking_score numeric, trending_score numeric,
                         trending_source text, age_credit numeric);
CREATE OR REPLACE FUNCTION pg_temp.snapshot(tag text) RETURNS void LANGUAGE sql AS $$
  DELETE FROM snaps WHERE snaps.tag = $1;
  INSERT INTO snaps SELECT $1, s.entity_id, s.ranking_score, s.trending_score, s.trending_source, s.age_credit
  FROM entity_ranking_scores s;
$$;
CREATE OR REPLACE FUNCTION pg_temp.same(a text, b text) RETURNS boolean LANGUAGE sql AS $$
  SELECT NOT EXISTS (
    (SELECT entity_id, ranking_score, trending_score, trending_source, age_credit FROM snaps WHERE tag = a
     EXCEPT SELECT entity_id, ranking_score, trending_score, trending_source, age_credit FROM snaps WHERE tag = b)
    UNION ALL
    (SELECT entity_id, ranking_score, trending_score, trending_source, age_credit FROM snaps WHERE tag = b
     EXCEPT SELECT entity_id, ranking_score, trending_score, trending_source, age_credit FROM snaps WHERE tag = a))
  AND (SELECT count(*) FROM snaps WHERE tag = a) > 10
$$;

INSERT INTO spaces (id, type, address)
SELECT id, 'DAO'::"spaceTypes", 'addr-' || k FROM ids WHERE k IN ('S1','S2','DEB');
INSERT INTO spaces (id, type, address)
SELECT id, 'Personal'::"spaceTypes", 'addr-' || k FROM ids WHERE k IN ('M1','M2','M3','N1','N2');

SELECT pg_temp.ent('UNL', pg_temp.noon(40)), pg_temp.typed('UNL', 'CLAIM');
SELECT pg_temp.ent('NOTD', pg_temp.noon(2)), pg_temp.typed('NOTD', 'CLAIM');
SELECT pg_temp.debate('PD', 1), pg_temp.debate('R1', 0), pg_temp.debate('R2', 3), pg_temp.debate('ROLD', 10),
       pg_temp.debate('BLK1', 2), pg_temp.debate('DUP', 2), pg_temp.debate('BAL', 20), pg_temp.debate('LPD', 5),
       pg_temp.debate('EARLY', 2), pg_temp.debate('NONMEM', 2), pg_temp.debate('ELSE', 2), pg_temp.debate('SUPER', 2),
       pg_temp.debate('UNR30', 31), pg_temp.debate('UNRNEW', 0), pg_temp.debate('D1', 30),
       pg_temp.debate('DL30', 30), pg_temp.debate('UOLD', 60);
-- GONE: typed Debate but no entities row.
SELECT pg_temp.typed('GONE', 'DEBATE');
INSERT INTO entity_feed_blocklist (entity_id, reason) VALUES (pg_temp.i('BLK1'), 'fixture');

SELECT pg_temp.agenda('A0', 'S1', 0);
SELECT pg_temp.names('A0', 'D1', 'S1', 'a0'), pg_temp.names('A0', 'LPD', 'S1', NULL), pg_temp.names('A0', 'DL30', 'S1', NULL);

-- The state before anything the ranking reads exists.
SELECT pg_temp.full_pass();
SELECT public.refresh_entity_ranking_scores(ARRAY(SELECT id FROM ids));
SELECT pg_temp.snapshot('before');
SELECT assert(pg_temp.trend('D1') = pg_temp.b() + pg_temp.p1() AND pg_temp.trend('R1') = 0,
              '0: (the fixture: the agenda places D1, nothing places R1)');

-- Everything the ranking reads.
INSERT INTO ranks.members (member_space_id, space_id) SELECT pg_temp.i(k), pg_temp.i('DEB') FROM unnest(ARRAY['M1','M2']) k;
INSERT INTO ranks.editors (member_space_id, space_id) VALUES (pg_temp.i('M3'), pg_temp.i('DEB'));
SELECT pg_temp.ballot('B1OLD', 'M1', 10, ARRAY['SUPER','PD']);
SELECT pg_temp.ballot('B1', 'M1', 20, ARRAY['PD','R1','R2','BAL']);
SELECT pg_temp.ballot('B2', 'M2', 15, ARRAY['R1','DUP']);
SELECT pg_temp.scored('PD', 1), pg_temp.scored('R1', 2), pg_temp.scored('R2', 3), pg_temp.scored('NOTD', 4),
       pg_temp.scored('ROLD', 5), pg_temp.scored('BLK1', 6), pg_temp.scored('DUP', 7), pg_temp.scored('BAL', 8),
       pg_temp.scored('DUP', 9, 'S2'), pg_temp.scored('GONE', 10);
SELECT pg_temp.down('M2', 'PD', 'DEB', 1), pg_temp.down('M3', 'PD', 'DEB', 1);
SELECT pg_temp.down('M2', 'LPD', 'DEB', 1), pg_temp.down('M3', 'LPD', 'DEB', 1);
SELECT pg_temp.down('M2', 'EARLY', 'DEB', 72), pg_temp.down('M3', 'EARLY', 'DEB', 72);
SELECT pg_temp.down('N1', 'NONMEM', 'DEB', 1), pg_temp.down('N2', 'NONMEM', 'S1', 1);
SELECT pg_temp.down('M3', 'ELSE', 'S2', 1);
SELECT pg_temp.down('M2', 'SUPER', 'DEB', 1);
SELECT pg_temp.down('M2', 'BAL', 'DEB', 1);
SELECT pg_temp.down('M2', 'DL30', 'DEB', 1), pg_temp.down('M3', 'DL30', 'DEB', 1);
INSERT INTO trending.ranking_place_sources (name, space_id, block_id, weight, window_days, slots, day_factor)
VALUES ('new', pg_temp.i('DEB'), pg_temp.i('BLK'), 1, 7, 20, 0.65),
       ('all_time', pg_temp.i('DEB'), pg_temp.i('BLK'), 0.5, NULL, 30, 0.97);
UPDATE entity_ranking_config SET ranking_pushdown_since = pg_temp.midnight(1) WHERE id;

-- ---------------------------------------------------------------------------
-- 0. Off.
-- ---------------------------------------------------------------------------
SELECT pg_temp.full_pass();
SELECT public.refresh_entity_ranking_scores(ARRAY(SELECT id FROM ids));
SELECT pg_temp.snapshot('off');
SELECT assert(pg_temp.same('before', 'off'),
              '0: off, with sources, ballots, scores and member downvotes present, every score is unchanged');
SELECT assert(NOT EXISTS (SELECT 1 FROM trending.ranking_places) AND NOT EXISTS (SELECT 1 FROM trending.ranking_pushdowns)
              AND NOT EXISTS (SELECT 1 FROM trending.ranking_shadow_report)
              AND NOT EXISTS (SELECT 1 FROM trending.ranking_shadow_days),
              '0: off keeps no places, push-downs or reports');
SELECT assert(trending.refresh_agenda_listings(false) = 0, '0: and an off hourly run re-scores nothing');

-- ---------------------------------------------------------------------------
-- 1. Shadow.
-- ---------------------------------------------------------------------------
UPDATE entity_ranking_config SET ranking_places_mode = 1 WHERE id;
SELECT pg_temp.full_pass();
SELECT public.refresh_entity_ranking_scores(ARRAY(SELECT id FROM ids));
SELECT pg_temp.snapshot('shadow');
SELECT assert(pg_temp.same('before', 'shadow'), '1: shadow moves no score');
SELECT assert(pg_temp.place('new', 'R1') = 1 AND pg_temp.place('all_time', 'R1') = 1 AND pg_temp.pushed('PD'),
              '1: while the places and push-downs are kept');
SELECT assert((SELECT ranking_rank FROM trending.ranking_shadow_report
               WHERE report_on = pg_temp.today() AND entity_id = pg_temp.i('R1')) = 1
              AND (SELECT served_rank FROM trending.ranking_shadow_report
                   WHERE report_on = pg_temp.today() AND entity_id = pg_temp.i('D1')) = 1,
              '1: the report holds the ranking''s top (R1) and Best''s (D1, the agenda''s first place)');
SELECT assert((SELECT count(*) FROM trending.ranking_shadow_report
               WHERE report_on = pg_temp.today() AND ranking_rank IS NOT NULL) = 5
              AND (SELECT count(*) FROM trending.ranking_shadow_report WHERE report_on = pg_temp.today())
                  = (SELECT count(*) FROM trending.rankable_debates(ARRAY(SELECT entity_id FROM entity_ranking_scores)))
              AND NOT EXISTS (SELECT 1 FROM trending.ranking_shadow_report
                              WHERE report_on = pg_temp.today() AND (served_rank IS NULL OR on_rank IS NULL)),
              '1: every placed debate under the ranking alone; every scored debate has a served and a served-if-on rank (fewer than 50 here)');
SELECT assert((pg_temp.report('R1')).on_rank = 1 AND (pg_temp.report('D1')).on_rank > 5
              AND (pg_temp.report('PD')).on_rank > (pg_temp.report('UNR30')).on_rank
              AND (pg_temp.report('DL30')).on_rank > (pg_temp.report('UOLD')).on_rank,
              '1: served-if-on: the ranking leads, the agenda''s first place drops below the placed debates, pushed-down debates sink');
SELECT assert(abs((pg_temp.report('UNRNEW')).on_rank - (pg_temp.report('UNRNEW')).served_rank)
                <= (SELECT count(*) FROM trending.ranking_shadow_report
                    WHERE report_on = pg_temp.today() AND (place_value IS NOT NULL OR pushed_down)),
              '1: an unplaced organic debate moves only by the placed and pushed debates around it, not to rank 51');
SELECT assert((SELECT mode = 1 AND placed = 5 AND pushed_down = 5 AND rankers = 2 AND all_time_top = 6
                      AND all_time_top_two_ballots = 1 AND overlap = (SELECT count(*) FROM trending.ranking_shadow_report
                                                                       WHERE report_on = pg_temp.today())
                      AND recent_debates = 10 AND recent_ranked = 4 AND pushed_above_unranked = 0
               FROM trending.ranking_shadow_days WHERE report_on = pg_temp.today()),
              '1: the day row: overlap of the two top 50s, places, push-downs, violations and the cutover bar');
SELECT assert((SELECT move_up FROM trending.ranking_shadow_moves
               WHERE report_on = pg_temp.today() AND entity_id = pg_temp.i('R1'))
              = (pg_temp.report('R1')).served_rank - 1,
              '1: and the moves view says how far each debate moves when on');
-- With no push-down size, a pushed-down debate stays above an unranked one of about its age: the report counts it.
UPDATE entity_ranking_config SET ranking_pushdown = 0 WHERE id;
SELECT trending.write_ranking_shadow_report(pg_temp.today());
SELECT assert((SELECT pushed_above_unranked FROM trending.ranking_shadow_days WHERE report_on = pg_temp.today()) > 0,
              '1: the report counts pushed-down debates still above an unranked debate created within 30 days');
UPDATE entity_ranking_config SET ranking_pushdown = 150 WHERE id;
SELECT trending.write_ranking_shadow_report(pg_temp.today());
SELECT assert(trending.refresh_agenda_listings(false) = 0, '1: a shadow hourly run re-scores nothing');

-- ---------------------------------------------------------------------------
-- 2. On: places.
-- ---------------------------------------------------------------------------
UPDATE entity_ranking_config SET ranking_places_mode = 2 WHERE id;
SELECT pg_temp.full_pass();
SELECT assert(pg_temp.place('new', 'R1') = 1 AND pg_temp.place('new', 'R2') = 2 AND pg_temp.place('new', 'DUP') = 3
              AND (SELECT count(*) FROM trending.ranking_places WHERE source = 'new') = 3,
              '2: New: the last 7 days'' debates, renumbered 1.. among them');
SELECT assert(pg_temp.place('all_time', 'R1') = 1 AND pg_temp.place('all_time', 'R2') = 2
              AND pg_temp.place('all_time', 'ROLD') = 3 AND pg_temp.place('all_time', 'DUP') = 4
              AND pg_temp.place('all_time', 'BAL') = 5
              AND (SELECT count(*) FROM trending.ranking_places WHERE source = 'all_time') = 5,
              '2: All time: no window; a non-debate, a blocklisted debate, a missing entity and a pushed-down debate take no place');
SELECT assert(pg_temp.trend('R1') = pg_temp.pv('new', 1, 0) AND pg_temp.src('R1') = 'listing',
              '2: a ranked debate with no agenda listing gets its place (reported as a listing)');
SELECT assert(pg_temp.near(pg_temp.trend('R2'), GREATEST(pg_temp.pv('new', 2, 3), pg_temp.pv('all_time', 2, 3)))
              AND pg_temp.pv('all_time', 2, 3) > pg_temp.pv('new', 2, 3),
              '2: in both rows, a debate keeps its best weighted place (All time''s here)');
SELECT assert(pg_temp.near(pg_temp.trend('ROLD'), pg_temp.pv('all_time', 3, 10))
              AND pg_temp.near(pg_temp.trend('DUP'), GREATEST(pg_temp.pv('new', 3, 2), pg_temp.pv('all_time', 4, 2))),
              '2: fading by the debate''s age; one place per entity across item spaces');
SELECT assert(pg_temp.trend('NOTD') = 0 AND pg_temp.trend('BLK1') = 0, '2: the non-debate and the blocklisted debate get nothing');
SELECT assert(pg_temp.trend('D1') = pg_temp.b(), '2: agendas stop setting places: D1 keeps only the plain listing boost');
SELECT assert(pg_temp.score('R1') > pg_temp.score('R2') AND pg_temp.score('R2') > pg_temp.score('DUP')
              AND pg_temp.score('DUP') > pg_temp.score('ROLD') AND pg_temp.score('ROLD') > pg_temp.score('D1'),
              '2: so the head of Best follows the ranking');

-- ---------------------------------------------------------------------------
-- 3. On: push-down.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.pushed('PD') AND pg_temp.place('new', 'PD') IS NULL AND pg_temp.place('all_time', 'PD') IS NULL
              AND pg_temp.trend('PD') = -(SELECT ranking_pushdown FROM entity_ranking_config WHERE id),
              '3: ranked #1 by one member and downvoted by two: no place and a flat -150');
SELECT assert((SELECT downvoters = 2 AND rankers = 1 FROM trending.ranking_pushdowns WHERE entity_id = pg_temp.i('PD')),
              '3: (two downvoters, one ranker)');
SELECT assert(pg_temp.trend('LPD') = pg_temp.b() - pg_temp.pd() AND pg_temp.src('LPD') = 'listing',
              '3: the push-down comes after the GREATEST: a listed debate keeps its boost, minus 150');
SELECT assert(pg_temp.credit('LPD') = 0 AND pg_temp.credit('DL30') = 0 AND pg_temp.credit('D1') > 0,
              '3: a pushed-down debate an agenda lists gets no age credit while on (an unpushed listed one keeps it)');
SELECT assert(pg_temp.score('DL30') < pg_temp.score('UOLD'),
              '3: a 30-day-old pushed-down debate listed today sits below an unranked debate created 30 days before it');
SELECT assert(NOT pg_temp.pushed('EARLY') AND pg_temp.trend('EARLY') = 0, '3: downvotes before the launch time do not count');
SELECT assert(NOT pg_temp.pushed('NONMEM'), '3: non-members'' downvotes do not count, even cast from the Debates space');
SELECT assert(pg_temp.pushed('ELSE'), '3: a member''s downvote counts whatever space it was cast from (user_votes.user_id, not space_id)');
SELECT assert(pg_temp.pushed('SUPER'), '3: a superseded ballot does not count as ranking it');
SELECT assert(NOT pg_temp.pushed('BAL') AND pg_temp.place('all_time', 'BAL') = 5,
              '3: as many rankers as downvoters is not a push-down');
SELECT assert(pg_temp.score('PD') < pg_temp.score('UNR30') - 0 AND pg_temp.score('PD') < pg_temp.score('UNRNEW')
              AND pg_temp.score('UNR30') - pg_temp.trend('UNR30') < pg_temp.score('PD') - pg_temp.trend('PD'),
              '3: a pushed-down debate sits below unranked debates created within 30 days of it, older ones included');
SELECT pg_temp.full_pass();
SELECT assert(pg_temp.trend('PD') = -pg_temp.pd(), '3: a full pass keeps the push-down');

-- A new member downvote: BAL goes from one ranker vs one downvoter to two downvoters.
SELECT pg_temp.down('M3', 'BAL', 'DEB', 0);
SELECT assert(trending.refresh_agenda_listings(false) = 1, '3: an hourly run re-scores just the debate pushed down');
SELECT assert(pg_temp.pushed('BAL') AND pg_temp.place('all_time', 'BAL') IS NULL AND pg_temp.trend('BAL') = -pg_temp.pd(),
              '3: and it loses its place within the hour');
SELECT assert(trending.refresh_agenda_listings(false) = 0, '3: an unchanged hour re-scores nothing');

-- A vote's own re-score keeps the terms.
SELECT public.refresh_entity_ranking_scores(ARRAY[pg_temp.i('R1'), pg_temp.i('PD')]);
SELECT assert(pg_temp.trend('R1') = pg_temp.pv('new', 1, 0) AND pg_temp.trend('PD') = -pg_temp.pd(),
              '3: a vote''s re-score keeps the place and the push-down');

-- Places come from the push-downs passed in: with none passed, PD would take New's first place.
SELECT assert(EXISTS (SELECT 1 FROM trending.current_ranking_places(ARRAY[]::uuid[]) p
                      WHERE p.source = 'new' AND p.entity_id = pg_temp.i('PD') AND p.place = 1)
              AND NOT EXISTS (SELECT 1 FROM trending.current_ranking_places() p WHERE p.entity_id = pg_temp.i('PD')),
              '3: current_ranking_places takes the push-downs as an argument (default: the current ones)');

-- A weight change waits for the full pass.
UPDATE trending.ranking_place_sources SET weight = 0 WHERE name = 'new';
SELECT pg_temp.full_pass();
SELECT assert(pg_temp.near(pg_temp.trend('R1'), pg_temp.pv('all_time', 1, 0)),
              '3: a weight change applies on the full pass');
UPDATE trending.ranking_place_sources SET weight = 1 WHERE name = 'new';
DELETE FROM user_votes WHERE user_id = pg_temp.i('M3') AND object_id = pg_temp.i('BAL');

-- ---------------------------------------------------------------------------
-- 4. Rollback.
-- ---------------------------------------------------------------------------
UPDATE entity_ranking_config SET ranking_places_mode = 1 WHERE id;
SELECT pg_temp.full_pass();
SELECT pg_temp.snapshot('rollback');
SELECT assert(pg_temp.same('before', 'rollback'), '4: back to shadow, a full pass gives back exactly the off scores');
UPDATE entity_ranking_config SET ranking_places_mode = 0 WHERE id;
SELECT pg_temp.full_pass();
SELECT pg_temp.snapshot('off_again');
SELECT assert(pg_temp.same('before', 'off_again')
              AND NOT EXISTS (SELECT 1 FROM trending.ranking_places) AND NOT EXISTS (SELECT 1 FROM trending.ranking_pushdowns),
              '4: and off empties the tables');

-- Cleanup for running by hand.
UPDATE entity_ranking_config SET ranking_places_mode = 0, ranking_pushdown_since = 0 WHERE id;
UPDATE trending.refresh_state SET last_full_pass_on = NULL WHERE id;
TRUNCATE entities, values, relations, votes_count, user_votes, spaces, entity_ranking_scores, entity_type_ranking,
         entity_topic_ranking, entity_feed_blocklist, trending.agenda_listings, trending.agenda_debate_positions,
         trending.ranking_place_sources, trending.ranking_places, trending.ranking_pushdowns,
         trending.ranking_shadow_report, trending.ranking_shadow_days, ranks.rankings, ranks.ranking_items,
         ranks.ranking_scores, ranks.members, ranks.editors CASCADE;
DROP TABLE ids;
DROP TABLE snaps;
