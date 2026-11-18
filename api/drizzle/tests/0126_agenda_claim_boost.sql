-- Assertions for 0126: agenda claims lift the best-ranked debate on them and drop its ranked siblings, while the
-- Best debates ranking is on (GEO-3781). Off by default.
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts. Expected values are written against
-- entity_ranking_config and the sources rows.
--
--   0. Ships off: both steps default to 0; the re-created functions still read the switch and the boosts table; the
--      wrapper still inlines (the core has not since 0127).
--   1. Zero config: with both steps at 0 (any day factor) and mode 2, no rows are stored and every score is the
--      0124 one (place value only). With steps above 0 in modes 0 and 1, still no rows and no score moves.
--   2. On: the best ranked debate on a listed claim gets the lift; its ranked siblings lose the drop, floored at the
--      place base; unranked debates are untouched; the lift and drop fade with the listing's Date; dates outside
--      the last 7 days do nothing; the newest Date wins and nothing stacks; debates named only by an agenda's
--      Debates relation get nothing; multi-claim debates take the drop only under a lifted #1; a step is one place
--      in the debate's own source; a listed claim typed Debate counts as a debate on itself, and a debate typed
--      Claim counts itself as one of its claims whether it is listed or not.
--   3. Push-down: a pushed-down debate is never #1 (the next ranked debate is) and stays below unranked debates.
--   4. Freshness: a removed Claims link is re-scored within the hour, the lift moving to the sibling; an unchanged
--      hour re-scores nothing.
--   5. Off again: steps back to 0, a non-forced hourly run empties the table and gives back the zero-config scores;
--      mode 1 gives back the shadow scores.

\set ON_ERROR_STOP on

-- No JIT here. The fixture tables are never analyzed, so the planner prices each re-score's entity_trending_terms_as
-- plan at ~10^8, and a JIT-enabled server (CI's postgres:16) compiles and optimizes that plan on every pass: ~45
-- times in this suite, which is nearly all of its CI time. JIT never changes a result. The runner's ROLLBACK undoes
-- the SET, so later suites keep the server default; run by hand, it lasts for the psql session.
SET jit = off;

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

-- 0. What ships.
SELECT assert((SELECT column_default FROM information_schema.columns
               WHERE table_name = 'entity_ranking_config' AND column_name = 'agenda_claim_steps') = '0'
              AND (SELECT column_default FROM information_schema.columns
                   WHERE table_name = 'entity_ranking_config' AND column_name = 'agenda_follower_drop_steps') = '0'
              AND (SELECT column_default FROM information_schema.columns
                   WHERE table_name = 'entity_ranking_config' AND column_name = 'agenda_day_factor') = '0.65',
              '0: both steps default to 0 (off); the day factor to 0.65');
SELECT assert(pg_get_functiondef('trending.entity_trending_terms_as(uuid[], boolean)'::regprocedure) LIKE '%agenda_boosts%'
              AND pg_get_functiondef('trending.entity_trending_terms_as(uuid[], boolean)'::regprocedure)
                    LIKE '%ranking_places_mode%'
              AND pg_get_functiondef('trending.refresh_agenda_listings(boolean)'::regprocedure) LIKE '%current_agenda_boosts()%'
              AND pg_get_functiondef('trending.refresh_agenda_listings(boolean)'::regprocedure) LIKE '%ranking_places_mode%',
              '0: the terms and the sweep still read the switch and the agenda boosts (a revert from 0124''s bodies fails here)');
CREATE OR REPLACE FUNCTION pg_temp.plan(q text) RETURNS text LANGUAGE plpgsql AS $$
DECLARE line text; out text := '';
BEGIN
  FOR line IN EXECUTE 'EXPLAIN ' || q LOOP out := out || line || E'\n'; END LOOP;
  RETURN out;
END $$;
SELECT assert(pg_temp.plan('SELECT * FROM trending.entity_trending_terms(''{00000000-0000-4000-8000-000000000001}''::uuid[])')
                !~ 'Function Scan on entity_trending_terms\s',
              '0: entity_trending_terms still inlines (the core is a Function Scan since 0127, for jit = off)');
SELECT assert(NOT EXISTS (SELECT 1 FROM trending.agenda_boosts), '0: the boosts table ships empty');
CREATE OR REPLACE FUNCTION pg_temp.rejected(sets text) RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN
  EXECUTE 'UPDATE entity_ranking_config SET ' || sets || ' WHERE id';
  RETURN false;
EXCEPTION WHEN check_violation THEN
  RETURN true;
END $$;
SELECT assert(pg_temp.rejected('agenda_day_factor = 0') AND pg_temp.rejected('agenda_day_factor = 1.5')
              AND pg_temp.rejected('agenda_claim_steps = -1') AND pg_temp.rejected('agenda_follower_drop_steps = -1'),
              '0: the config CHECK rejects a day factor outside (0, 1] and negative steps');

TRUNCATE entities, values, relations, votes_count, user_votes, spaces, entity_ranking_scores,
         entity_type_weights, entity_type_exclusions, entity_type_ranking, entity_topic_ranking,
         entity_feed_blocklist, personalization.feed_item_signals_hourly, trending.agenda_listings,
         trending.agenda_debate_positions, trending.ranking_place_sources, trending.ranking_places,
         trending.ranking_pushdowns, trending.ranking_shadow_report, trending.ranking_shadow_days,
         trending.agenda_boosts, ranks.rankings, ranks.ranking_items, ranks.ranking_scores, ranks.members,
         ranks.editors CASCADE;
UPDATE trending.refresh_state SET last_full_pass_on = NULL WHERE id;

UPDATE entity_ranking_config
   SET participation_weight = 4, participation_cap = 19, comment_weight = 0, comment_cap = 30,
       tau_seconds = 100000, trending_boost = 8, trending_topic_boost = 2.5, trending_day_factor = 0.65,
       trending_debate_factor = 0.75, trending_debate_window_days = 7, trending_horizon_days = 16,
       trending_position_base = 2500, trending_position_step = 100, trending_position_slots = 50,
       trending_position_day_factor = 0.3,
       ranking_places_mode = 0, ranking_place_base = 2500, ranking_place_step = 100, ranking_pushdown = 150,
       ranking_pushdown_since = 0, agenda_claim_steps = 0, agenda_follower_drop_steps = 0, agenda_day_factor = 0.65
 WHERE id;

-- ---------------------------------------------------------------------------
-- Fixtures. Days are New York calendar days before today; debates are created at noon, 20 days ago unless said.
-- The Best debates block BLK (one source 'all_time', weight 1, 100 slots, no fade) ranks, by position:
--    2 P1    on C7, downvoted by M2 and M3 (no ballot): pushed down, so no place
--    3 A1    on C1 (listed today in S1)                       → #1 on C1: lifted
--    4 MD    on C1 and C9 (C9 listed, nothing else ranked on it) → drop on C1 (C1 has a lifted #1); nothing on C9
--    5 A2    on C1                                            → #3 (under MD): dropped
--    6 MD2   on C5 (listed, only unranked debates) and UNL    → nothing
--    7 D9    on UNL (unlisted); named by agenda AD's Debates relation → nothing
--   10 B1    on C2 (listed 3 days ago in S1 and today in S2)  → lifted, the newest Date (today)
--   20 E1    on C6 (listed 2 days ago)                        → lifted, faded 2 days
--   30 P2    on C7 (listed today)                             → #1 once P1 is pushed down
--   40 CD    typed Claim and Debate, itself listed today      → lifted as a debate on itself
--   50 OLD1  on C3 (listed 7 days ago: outside)               → nothing
--   60 FUT1  on C4 (listed tomorrow)                          → nothing
--   95 A3    on C1                                            → #4: dropped, floored at the place base
--   96 F1    on C11 (listed 6 days ago: the window's last day) → lifted, faded 6 days
--   97 F2    on C11                                           → #2: dropped, faded 6 days
--   98 MG    on C12 (listed today) and UNL                    → drop on C12 though it outranks G1: rank 2, never 1
--   99 G1    on C12                                           → #1 on C12 (MG is multi-claim): lifted
--  100 CDX   typed Claim and Debate, unlisted, on C14 (listed today) → multi-claim (itself and C14): drop on C14
--  101 G2    on C14                                           → #1 on C14 though CDX outranks it: lifted
-- Block BLK2 (source 'half', weight 0.5, 100 slots): 1 H on C10 (listed today) → lifted by half the points;
--   2 W on C13 (listed today), also 1 in BLK4 (source 'faded', weight 1, fade 0.5 a day: about 0 after 20 days) →
--   its value comes from 'half', so its lift is half the points, not the heavier source's full points.
-- Block BLK3 (source 'tenth', weight 0.1, 100 slots): 1 H2 on C10 → #2 under H: dropped, but its place value (1250)
--   is already below the place base, so the floor leaves it there (never raises it).
-- Unranked: U1 on C1, U5 on C5, UNR (on UNL, 1 day old) and UOLD (on UNL, 25 days old).
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE ids AS
SELECT k, ('b1260000-0000-4000-8000-' || lpad(to_hex(n), 12, '0'))::uuid AS id
FROM unnest(ARRAY['UNL','C1','C2','C3','C4','C5','C6','C7','C9','C10','S1','S2','DEB','BLK','BLK2','M2','M3',
                  'A0','A2D','A2T','AOLD','AFUT','A6','AD',
                  'P1','A1','MD','A2','MD2','D9','B1','E1','P2','CD','OLD1','FUT1','A3','H','U1','U5','UNR','UOLD',
                  'BLK3','H2','C11','C12','C13','A6B','F1','F2','MG','G1','W','BLK4','C14','CDX','G2'])
     WITH ORDINALITY AS u(k, n)
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
CREATE OR REPLACE FUNCTION pg_temp.claims(k text, claim_k text) RETURNS void LANGUAGE sql AS $$
  SELECT pg_temp.rel(k, 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid, claim_k, 'S1') $$;
CREATE OR REPLACE FUNCTION pg_temp.claim(k text) RETURNS void LANGUAGE sql AS $$
  SELECT pg_temp.ent(k, pg_temp.noon(40)); SELECT pg_temp.typed(k, 'CLAIM') $$;
CREATE OR REPLACE FUNCTION pg_temp.debate(k text, claim_k text, d integer DEFAULT 20) RETURNS void LANGUAGE sql AS $$
  SELECT pg_temp.ent(k, pg_temp.noon(d)); SELECT pg_temp.typed(k, 'DEBATE'); SELECT pg_temp.claims(k, claim_k) $$;
-- A Debate agenda in `space_k` dated `d` days ago (negative: in the future).
CREATE OR REPLACE FUNCTION pg_temp.agenda(k text, space_k text, d integer) RETURNS void LANGUAGE sql AS $$
  INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
  VALUES (pg_temp.i(k), '1790000000', '0', '1790000000', '0');
  SELECT pg_temp.rel(k, '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid, 'AGENDA', space_k);
  INSERT INTO values (id, entity_id, property_id, space_id, datetime, datetime_utc)
  VALUES (gen_random_uuid()::text, pg_temp.i(k), '52665f3e-fb7d-48d5-8b6b-abb21b0d36db'::uuid, pg_temp.i(space_k),
          'x', to_timestamp(pg_temp.midnight(d)));
$$;
CREATE OR REPLACE FUNCTION pg_temp.lists(agenda_k text, claim_k text, space_k text) RETURNS void LANGUAGE sql AS $$
  SELECT pg_temp.rel(agenda_k, 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid, claim_k, space_k) $$;
CREATE OR REPLACE FUNCTION pg_temp.scored(blk text, k text, pos integer) RETURNS void LANGUAGE sql AS $$
  INSERT INTO ranks.ranking_scores (block_id, entity_id, space_id, score, position)
  VALUES (pg_temp.i(blk), pg_temp.i(k), pg_temp.i('S1'), 1.0 / pos, pos)
$$;
CREATE OR REPLACE FUNCTION pg_temp.down(who text, k text) RETURNS void LANGUAGE sql AS $$
  INSERT INTO user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at)
  VALUES (pg_temp.i(who), pg_temp.i(k), 0, pg_temp.i('DEB'), 1, 0, now() - interval '1 hour')
$$;

CREATE OR REPLACE FUNCTION pg_temp.score(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT ranking_score FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k) $$;
CREATE OR REPLACE FUNCTION pg_temp.trend(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT COALESCE((SELECT trending_score FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k)), 0) $$;
CREATE OR REPLACE FUNCTION pg_temp.boost(k text) RETURNS trending.agenda_boosts LANGUAGE sql AS $$
  SELECT * FROM trending.agenda_boosts WHERE entity_id = pg_temp.i(k) $$;
CREATE OR REPLACE FUNCTION pg_temp.near(a numeric, b numeric) RETURNS boolean LANGUAGE sql AS $$
  SELECT abs(a - b) < 1e-9 $$;
-- A debate's weighted place value (0124's), from its stored place. Places are renumbered among rankable debates,
-- so A1 (position 3) is place 1, P1 being pushed down.
CREATE OR REPLACE FUNCTION pg_temp.v(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT place_value FROM trending.ranking_place_values(ARRAY[pg_temp.i(k)]) $$;
-- n steps in a source of weight w, faded d days.
CREATE OR REPLACE FUNCTION pg_temp.steps(n numeric, w numeric, d integer) RETURNS numeric LANGUAGE sql AS $$
  SELECT n * c.ranking_place_step * w * power(c.agenda_day_factor, d) FROM entity_ranking_config c WHERE c.id $$;
CREATE OR REPLACE FUNCTION pg_temp.lift_steps() RETURNS numeric LANGUAGE sql AS $$
  SELECT agenda_claim_steps FROM entity_ranking_config WHERE id $$;
CREATE OR REPLACE FUNCTION pg_temp.drop_steps() RETURNS numeric LANGUAGE sql AS $$
  SELECT agenda_follower_drop_steps FROM entity_ranking_config WHERE id $$;
CREATE OR REPLACE FUNCTION pg_temp.b() RETURNS numeric LANGUAGE sql AS $$
  SELECT trending_boost FROM entity_ranking_config WHERE id $$;
-- How many claims a debate is on: its Claims links that count, and itself when typed Claim and Debate.
CREATE OR REPLACE FUNCTION pg_temp.nclaims(d uuid) RETURNS integer LANGUAGE sql AS $$
  SELECT count(*)::integer FROM (
    SELECT l.claim_id FROM trending.debate_links('{}', ARRAY[d]) l
    UNION SELECT k.entity_id FROM trending.claims_and_debates(ARRAY[d]) k WHERE k.is_claim AND k.is_debate) x $$;
-- The stored numbering: per claim the ranks are distinct and run 1..n, exactly one row is rank 1, it alone has a
-- lift, and it is never a debate on several claims.
CREATE OR REPLACE FUNCTION pg_temp.ranks_ok() RETURNS boolean LANGUAGE sql AS $$
  SELECT NOT EXISTS (
           SELECT 1 FROM trending.agenda_boosts b GROUP BY b.claim_id
           HAVING count(DISTINCT b.rank_on_claim) <> count(*) OR min(b.rank_on_claim) <> 1
               OR max(b.rank_on_claim) <> count(*) OR count(*) FILTER (WHERE b.rank_on_claim = 1) <> 1
               OR count(*) FILTER (WHERE b.lift > 0) <> 1 OR bool_or(b.rank_on_claim = 1 AND b.lift <= 0))
     AND NOT EXISTS (SELECT 1 FROM trending.agenda_boosts b
                     WHERE b.rank_on_claim = 1 AND pg_temp.nclaims(b.entity_id) > 1) $$;
CREATE OR REPLACE FUNCTION pg_temp.full_pass() RETURNS integer LANGUAGE sql AS $$
  SELECT trending.refresh_agenda_listings(true) $$;
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
-- Re-score everything and snapshot: what a forced full pass plus every vote's own re-score would store.
CREATE OR REPLACE FUNCTION pg_temp.settle(tag text) RETURNS void LANGUAGE sql AS $$
  SELECT pg_temp.full_pass();
  SELECT public.refresh_entity_ranking_scores(ARRAY(SELECT id FROM ids));
  SELECT pg_temp.snapshot(tag);
$$;

INSERT INTO spaces (id, type, address)
SELECT id, 'DAO'::"spaceTypes", 'addr-' || k FROM ids WHERE k IN ('S1','S2','DEB');
INSERT INTO spaces (id, type, address)
SELECT id, 'Personal'::"spaceTypes", 'addr-' || k FROM ids WHERE k IN ('M2','M3');

SELECT pg_temp.claim(k) FROM unnest(ARRAY['UNL','C1','C2','C3','C4','C5','C6','C7','C9','C10','C11','C12','C13','C14']) k;
SELECT pg_temp.debate('P1', 'C7'), pg_temp.debate('A1', 'C1'), pg_temp.debate('MD', 'C1'), pg_temp.claims('MD', 'C9'),
       pg_temp.debate('A2', 'C1'), pg_temp.debate('MD2', 'C5'), pg_temp.claims('MD2', 'UNL'),
       pg_temp.debate('D9', 'UNL'), pg_temp.debate('B1', 'C2'), pg_temp.debate('E1', 'C6'), pg_temp.debate('P2', 'C7'),
       pg_temp.debate('OLD1', 'C3'), pg_temp.debate('FUT1', 'C4'), pg_temp.debate('A3', 'C1'), pg_temp.debate('H', 'C10'),
       pg_temp.debate('U1', 'C1'), pg_temp.debate('U5', 'C5'), pg_temp.debate('UNR', 'UNL', 1),
       pg_temp.debate('UOLD', 'UNL', 25), pg_temp.debate('H2', 'C10'), pg_temp.debate('F1', 'C11'),
       pg_temp.debate('F2', 'C11'), pg_temp.debate('MG', 'C12'), pg_temp.claims('MG', 'UNL'), pg_temp.debate('G1', 'C12'),
       pg_temp.debate('W', 'C13'), pg_temp.debate('G2', 'C14');
SELECT pg_temp.ent('CD', pg_temp.noon(20)), pg_temp.typed('CD', 'CLAIM'), pg_temp.typed('CD', 'DEBATE');
SELECT pg_temp.ent('CDX', pg_temp.noon(20)), pg_temp.typed('CDX', 'CLAIM'), pg_temp.typed('CDX', 'DEBATE'),
       pg_temp.claims('CDX', 'C14');

SELECT pg_temp.agenda('A0', 'S1', 0);
SELECT pg_temp.lists('A0', k, 'S1') FROM unnest(ARRAY['C1','C5','C7','C9','C10','CD','C12','C13','C14']) k;
SELECT pg_temp.agenda('A2D', 'S1', 3), pg_temp.lists('A2D', 'C2', 'S1');
SELECT pg_temp.agenda('A2T', 'S2', 0), pg_temp.lists('A2T', 'C2', 'S2');
SELECT pg_temp.agenda('A6', 'S1', 2), pg_temp.lists('A6', 'C6', 'S1');
SELECT pg_temp.agenda('A6B', 'S1', 6), pg_temp.lists('A6B', 'C11', 'S1');
SELECT pg_temp.agenda('AOLD', 'S1', 7), pg_temp.lists('AOLD', 'C3', 'S1');
SELECT pg_temp.agenda('AFUT', 'S1', -1), pg_temp.lists('AFUT', 'C4', 'S1');
SELECT pg_temp.agenda('AD', 'S1', 0),
       pg_temp.rel('AD', 'b96bf701-a399-430d-a072-f5e910cdeda9'::uuid, 'D9', 'S1');          -- Debates

INSERT INTO ranks.members (member_space_id, space_id) SELECT pg_temp.i(k), pg_temp.i('DEB') FROM unnest(ARRAY['M2','M3']) k;
SELECT pg_temp.down('M2', 'P1'), pg_temp.down('M3', 'P1');
SELECT pg_temp.scored('BLK', 'P1', 2), pg_temp.scored('BLK', 'A1', 3), pg_temp.scored('BLK', 'MD', 4),
       pg_temp.scored('BLK', 'A2', 5), pg_temp.scored('BLK', 'MD2', 6), pg_temp.scored('BLK', 'D9', 7),
       pg_temp.scored('BLK', 'B1', 10), pg_temp.scored('BLK', 'E1', 20), pg_temp.scored('BLK', 'P2', 30),
       pg_temp.scored('BLK', 'CD', 40), pg_temp.scored('BLK', 'OLD1', 50), pg_temp.scored('BLK', 'FUT1', 60),
       pg_temp.scored('BLK', 'A3', 95), pg_temp.scored('BLK2', 'H', 1),
       pg_temp.scored('BLK3', 'H2', 1), pg_temp.scored('BLK', 'F1', 96), pg_temp.scored('BLK', 'F2', 97),
       pg_temp.scored('BLK', 'MG', 98), pg_temp.scored('BLK', 'G1', 99), pg_temp.scored('BLK2', 'W', 2),
       pg_temp.scored('BLK', 'CDX', 100), pg_temp.scored('BLK', 'G2', 101),
       pg_temp.scored('BLK4', 'W', 1);
INSERT INTO trending.ranking_place_sources (name, space_id, block_id, weight, window_days, slots, day_factor)
VALUES ('all_time', pg_temp.i('DEB'), pg_temp.i('BLK'), 1, NULL, 100, 1),
       ('half', pg_temp.i('DEB'), pg_temp.i('BLK2'), 0.5, NULL, 100, 1),
       ('tenth', pg_temp.i('DEB'), pg_temp.i('BLK3'), 0.1, NULL, 100, 1),
       ('faded', pg_temp.i('DEB'), pg_temp.i('BLK4'), 1, NULL, 100, 0.5);
UPDATE entity_ranking_config SET ranking_pushdown_since = pg_temp.midnight(1) WHERE id;

-- ---------------------------------------------------------------------------
-- 1. Zero config, every mode.
-- ---------------------------------------------------------------------------
SELECT pg_temp.settle('off');
SELECT assert(pg_temp.trend('A1') = pg_temp.b(), '1: (off: A1 carries only its claim''s plain listing boost)');
UPDATE entity_ranking_config SET agenda_claim_steps = 30, agenda_follower_drop_steps = 40 WHERE id;
SELECT pg_temp.settle('off_steps');
SELECT assert(pg_temp.same('off', 'off_steps') AND NOT EXISTS (SELECT 1 FROM trending.agenda_boosts),
              '1: mode 0 with steps above 0: no rows, no score moves');
UPDATE entity_ranking_config SET agenda_claim_steps = 0, agenda_follower_drop_steps = 0, ranking_places_mode = 1 WHERE id;
SELECT pg_temp.settle('shadow');
UPDATE entity_ranking_config SET agenda_claim_steps = 30, agenda_follower_drop_steps = 40 WHERE id;
SELECT pg_temp.settle('shadow_steps');
SELECT assert(pg_temp.same('shadow', 'shadow_steps') AND pg_temp.same('off', 'shadow')
              AND NOT EXISTS (SELECT 1 FROM trending.agenda_boosts),
              '1: mode 1 with steps above 0: no rows, no score moves');
UPDATE entity_ranking_config SET agenda_claim_steps = 0, agenda_follower_drop_steps = 0, ranking_places_mode = 2 WHERE id;
SELECT pg_temp.settle('zero');
SELECT assert(NOT EXISTS (SELECT 1 FROM trending.agenda_boosts), '1: mode 2 with both steps at 0 stores no rows');
SELECT assert(pg_temp.trend('A1') = pg_temp.v('A1') AND pg_temp.trend('A2') = pg_temp.v('A2')
              AND pg_temp.trend('A3') = pg_temp.v('A3') AND pg_temp.trend('H') = pg_temp.v('H')
              AND pg_temp.trend('U1') = pg_temp.b() AND pg_temp.trend('P1') = pg_temp.b() - 150,
              '1: and every score is 0124''s: the place value, the plain boost, the push-down');
UPDATE entity_ranking_config SET agenda_day_factor = 0.3 WHERE id;
SELECT pg_temp.settle('zero_factor');
SELECT assert(pg_temp.same('zero', 'zero_factor') AND NOT EXISTS (SELECT 1 FROM trending.agenda_boosts),
              '1: changing the day factor alone moves nothing');
UPDATE entity_ranking_config SET agenda_day_factor = 0.65 WHERE id;

-- ---------------------------------------------------------------------------
-- 2. On.
-- ---------------------------------------------------------------------------
UPDATE entity_ranking_config SET agenda_claim_steps = 30, agenda_follower_drop_steps = 40 WHERE id;
SELECT pg_temp.full_pass();
SELECT assert((pg_temp.boost('A1')).rank_on_claim = 1 AND (pg_temp.boost('A1')).claim_id = pg_temp.i('C1')
              AND pg_temp.trend('A1') = pg_temp.v('A1') + pg_temp.steps(pg_temp.lift_steps(), 1, 0),
              '2: the best ranked debate on a claim listed today gets the full lift on top of its place value');
SELECT assert((pg_temp.boost('A2')).rank_on_claim = 3
              AND pg_temp.trend('A2') = pg_temp.v('A2') - pg_temp.steps(pg_temp.drop_steps(), 1, 0),
              '2: the next ranked debate on it loses the drop from its place value');
SELECT assert((pg_temp.boost('A3')).rank_on_claim = 4
              AND pg_temp.trend('A3') = pg_temp.v('A3') - pg_temp.steps(pg_temp.drop_steps(), 1, 0),
              '2: and so does every ranked debate after it');
-- A drop bigger than the place values: floored at the place base.
UPDATE entity_ranking_config SET agenda_follower_drop_steps = 200 WHERE id;
SELECT pg_temp.full_pass();
SELECT assert(pg_temp.v('A3') - pg_temp.steps(pg_temp.drop_steps(), 1, 0) < 2500
              AND pg_temp.trend('A3') = 2500 AND pg_temp.trend('A2') = 2500,
              '2: a drop never takes a place value below the place base');
SELECT assert(pg_temp.score('A3') > pg_temp.score('U1') AND pg_temp.score('A3') > pg_temp.score('UNR'),
              '2: so every dropped sibling stays above the unranked debates');
SELECT assert((pg_temp.boost('H2')).rank_on_claim = 2 AND (pg_temp.boost('H2')).place_drop = 200 * 100 * 0.1
              AND pg_temp.v('H2') < 2500 AND pg_temp.trend('H2') = pg_temp.v('H2'),
              '2: a follower whose place value is already below the base keeps it (the floor never raises it)');
UPDATE entity_ranking_config SET agenda_follower_drop_steps = 40 WHERE id;
SELECT pg_temp.full_pass();
SELECT assert(pg_temp.boost('U1') IS NULL AND pg_temp.trend('U1') = pg_temp.b()
              AND pg_temp.boost('U5') IS NULL AND pg_temp.trend('U5') = pg_temp.b(),
              '2: unranked debates on listed claims are untouched (a claim with only unranked debates stores nothing)');
SELECT assert(pg_temp.score('A2') < pg_temp.score('E1') AND pg_temp.score('A1') > pg_temp.score('MD2'),
              '2: the dropped sibling (place 5) falls below place 20; the lifted one passes places it trailed');
SELECT assert(pg_temp.near(pg_temp.trend('E1'), pg_temp.v('E1') + pg_temp.steps(pg_temp.lift_steps(), 1, 2))
              AND (pg_temp.boost('E1')).listed_on = pg_temp.today() - 2,
              '2: the lift fades with the listing''s Date (0.65 a day)');
SELECT assert((pg_temp.boost('F1')).listed_on = pg_temp.today() - 6
              AND pg_temp.near(pg_temp.trend('F1'), pg_temp.v('F1') + pg_temp.steps(pg_temp.lift_steps(), 1, 6))
              AND (pg_temp.boost('F2')).rank_on_claim = 2
              AND pg_temp.near(pg_temp.trend('F2'), pg_temp.v('F2') - pg_temp.steps(pg_temp.drop_steps(), 1, 6)),
              '2: a listing 6 days old still counts, and the drop fades with the Date as the lift does');
SELECT assert(pg_temp.boost('OLD1') IS NULL AND pg_temp.trend('OLD1') = pg_temp.v('OLD1'),
              '2: a listing 7 days old does nothing');
SELECT assert(pg_temp.boost('FUT1') IS NULL AND pg_temp.trend('FUT1') = pg_temp.v('FUT1'),
              '2: a future Date does nothing until it arrives');
SELECT assert((pg_temp.boost('B1')).listed_on = pg_temp.today()
              AND pg_temp.trend('B1') = pg_temp.v('B1') + pg_temp.steps(pg_temp.lift_steps(), 1, 0),
              '2: listed by two agendas in two spaces, the newest Date counts and the lifts do not add up');
SELECT assert(pg_temp.boost('D9') IS NULL AND pg_temp.trend('D9') = pg_temp.v('D9'),
              '2: a debate an agenda names only through its Debates relation gets nothing');
SELECT assert((pg_temp.boost('MD')).claim_id = pg_temp.i('C1') AND (pg_temp.boost('MD')).lift = 0
              AND (pg_temp.boost('MD')).rank_on_claim = 2
              AND pg_temp.trend('MD') = pg_temp.v('MD') - pg_temp.steps(pg_temp.drop_steps(), 1, 0),
              '2: a multi-claim debate is never lifted, and drops under a claim with a lifted #1 (not C9, which has none)');
SELECT assert(pg_temp.boost('MD2') IS NULL AND pg_temp.trend('MD2') = pg_temp.v('MD2'),
              '2: a multi-claim debate on a claim with no lifted #1 gets nothing');
SELECT assert(pg_temp.v('MG') > pg_temp.v('G1')
              AND (pg_temp.boost('G1')).rank_on_claim = 1 AND (pg_temp.boost('G1')).lift > 0
              AND (pg_temp.boost('MG')).rank_on_claim = 2 AND (pg_temp.boost('MG')).lift = 0
              AND pg_temp.trend('MG') = pg_temp.v('MG') - pg_temp.steps(pg_temp.drop_steps(), 1, 0),
              '2: a multi-claim debate above the lifted #1 still takes the drop, stored as rank 2 (rank 1 is the lifted one)');
SELECT assert(pg_temp.trend('H') = pg_temp.v('H') + pg_temp.steps(pg_temp.lift_steps(), 0.5, 0)
              AND (pg_temp.boost('H')).lift = 30 * 100 * 0.5,
              '2: a step is one place in the debate''s own source: weight 0.5 halves the points');
SELECT assert(EXISTS (SELECT 1 FROM trending.ranking_places WHERE entity_id = pg_temp.i('W') AND source = 'faded')
              AND pg_temp.v('W') = 0.5 * (2500 + 100 * 99)
              AND (pg_temp.boost('W')).lift = 30 * 100 * 0.5
              AND pg_temp.trend('W') = pg_temp.v('W') + pg_temp.steps(pg_temp.lift_steps(), 0.5, 0),
              '2: in two sources, the step is the one whose weighted value counts, not the heavier source''s');
SELECT assert((pg_temp.boost('CD')).claim_id = pg_temp.i('CD') AND (pg_temp.boost('CD')).rank_on_claim = 1
              AND pg_temp.trend('CD') = pg_temp.v('CD') + pg_temp.steps(pg_temp.lift_steps(), 1, 0),
              '2: a listed claim typed Debate counts as a debate on itself');
SELECT assert(pg_temp.v('CDX') > pg_temp.v('G2') AND (pg_temp.boost('G2')).rank_on_claim = 1
              AND pg_temp.trend('G2') = pg_temp.v('G2') + pg_temp.steps(pg_temp.lift_steps(), 1, 0)
              AND (pg_temp.boost('CDX')).rank_on_claim = 2 AND (pg_temp.boost('CDX')).lift = 0
              AND pg_temp.trend('CDX') = pg_temp.v('CDX') - pg_temp.steps(pg_temp.drop_steps(), 1, 0),
              '2: an unlisted debate typed Claim counts itself as a claim: on one other claim it is multi-claim and drops');
SELECT pg_temp.lists('A0', 'CDX', 'S1');
SELECT pg_temp.full_pass();
SELECT assert((pg_temp.boost('CDX')).claim_id = pg_temp.i('C14') AND (pg_temp.boost('CDX')).rank_on_claim = 2
              AND (pg_temp.boost('G2')).rank_on_claim = 1,
              '2: and listing it too changes nothing (it is still multi-claim, with no other debate on itself)');
DELETE FROM relations WHERE from_entity_id = pg_temp.i('A0') AND to_entity_id = pg_temp.i('CDX');
SELECT pg_temp.full_pass();
SELECT assert((SELECT count(*) FROM trending.agenda_boosts) = 17
              AND NOT EXISTS (SELECT 1 FROM trending.agenda_boosts WHERE lift <= 0 AND place_drop <= 0),
              '2: seventeen rows (A1, A2, A3, MD, B1, E1, P2, CD, H, H2, F1, F2, MG, G1, W, CDX, G2), each with a lift or a drop');
SELECT assert(pg_temp.ranks_ok(),
              '2: per claim the ranks run 1..n with no repeats, and only the one rank 1 (never multi-claim) is lifted');
-- Listing every claim the ranked debates are on (and every Claim+Debate itself) never moves a debate between
-- single-claim and multi-claim: every lifted debate stays lifted on the same claim, every dropped one stays dropped.
CREATE TEMP TABLE base_boosts AS SELECT * FROM trending.agenda_boosts;
CREATE TEMP TABLE extra_listed AS
SELECT c.claim_id FROM (
  SELECT l.claim_id FROM trending.debate_links('{}', ARRAY(SELECT entity_id FROM trending.ranking_places)) l
  UNION SELECT k.entity_id FROM trending.claims_and_debates(ARRAY(SELECT entity_id FROM trending.ranking_places)) k
  WHERE k.is_claim AND k.is_debate) c
WHERE NOT EXISTS (SELECT 1 FROM relations r WHERE r.from_entity_id = pg_temp.i('A0') AND r.to_entity_id = c.claim_id);
INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
SELECT gen_random_uuid(), gen_random_uuid(), 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid, pg_temp.i('A0'), claim_id,
       pg_temp.i('S1'), false FROM extra_listed;
SELECT pg_temp.full_pass();
SELECT assert((SELECT count(*) FROM extra_listed) >= 6
              AND NOT EXISTS (SELECT 1 FROM base_boosts o LEFT JOIN trending.agenda_boosts n USING (entity_id)
                              WHERE n.entity_id IS NULL OR (o.lift > 0) <> (n.lift > 0)
                                 OR (o.lift > 0 AND (n.claim_id <> o.claim_id OR n.rank_on_claim <> 1))
                                 OR (o.lift = 0 AND n.place_drop <= 0))
              AND pg_temp.ranks_ok(),
              '2: listing every claim of the ranked debates keeps each lifted debate lifted and each dropped one dropped');
DELETE FROM relations WHERE from_entity_id = pg_temp.i('A0') AND to_entity_id IN (SELECT claim_id FROM extra_listed);
SELECT pg_temp.full_pass();
SELECT assert((SELECT count(*) FROM trending.agenda_boosts) = 17
              AND NOT EXISTS (SELECT entity_id, claim_id, rank_on_claim, listed_on, lift, place_drop FROM base_boosts
                              EXCEPT SELECT entity_id, claim_id, rank_on_claim, listed_on, lift, place_drop
                              FROM trending.agenda_boosts),
              '2: (listings removed again: the same rows as before)');
-- MD (place 2) also on C11 (listed 6 days ago, F1 #1 at place 96): it drops under both C1 and C11 and is kept under
-- C1 (today's larger drop), so C11's numbering is F1, F2 with no gap where MD would have been.
SELECT pg_temp.claims('MD', 'C11');
SELECT pg_temp.full_pass();
SELECT assert((pg_temp.boost('MD')).claim_id = pg_temp.i('C1') AND (pg_temp.boost('F1')).rank_on_claim = 1
              AND (pg_temp.boost('F2')).rank_on_claim = 2 AND pg_temp.ranks_ok(),
              '2: a multi-claim debate kept under one claim leaves no gap in another claim''s numbering');
DELETE FROM relations WHERE from_entity_id = pg_temp.i('MD') AND to_entity_id = pg_temp.i('C11');
SELECT pg_temp.full_pass();
-- A vote's own re-score of one debate reads the stored row: the lift is never handed to two debates.
SELECT public.refresh_entity_ranking_scores(ARRAY[pg_temp.i('A1'), pg_temp.i('A2')]);
SELECT assert(pg_temp.trend('A1') = pg_temp.v('A1') + pg_temp.steps(pg_temp.lift_steps(), 1, 0)
              AND pg_temp.trend('A2') = pg_temp.v('A2') - pg_temp.steps(pg_temp.drop_steps(), 1, 0),
              '2: a vote''s own re-score keeps the stored lift and drop');

-- ---------------------------------------------------------------------------
-- 3. Push-down.
-- ---------------------------------------------------------------------------
SELECT assert(pg_temp.boost('P1') IS NULL AND (pg_temp.boost('P2')).rank_on_claim = 1
              AND pg_temp.trend('P2') = pg_temp.v('P2') + pg_temp.steps(pg_temp.lift_steps(), 1, 0),
              '3: a pushed-down debate is never #1 on its claim: the next ranked debate is lifted');
SELECT assert(pg_temp.trend('P1') = pg_temp.b() - 150 AND pg_temp.score('P1') < pg_temp.score('UNR')
              AND pg_temp.score('P1') < pg_temp.score('UOLD'),
              '3: and it stays below unranked debates, its claim on the agenda or not');

-- ---------------------------------------------------------------------------
-- 4. Freshness.
-- ---------------------------------------------------------------------------
SELECT assert(trending.refresh_agenda_listings(false) = 0, '4: an unchanged hour re-scores nothing');
-- A1 loses its Claims link to C1: within the hour A1 loses its lift and A2 (next on C1) takes it.
DELETE FROM relations WHERE from_entity_id = pg_temp.i('A1') AND to_entity_id = pg_temp.i('C1');
SELECT assert(trending.refresh_agenda_listings(false) > 0, '4: a removed Claims link is picked up by the hourly run');
SELECT assert(pg_temp.boost('A1') IS NULL AND pg_temp.trend('A1') = pg_temp.v('A1')
              AND (pg_temp.boost('A2')).rank_on_claim = 1
              AND pg_temp.trend('A2') = pg_temp.v('A2') + pg_temp.steps(pg_temp.lift_steps(), 1, 0)
              AND (pg_temp.boost('MD')).rank_on_claim = 2 AND (pg_temp.boost('A3')).rank_on_claim = 3,
              '4: and the lift moves to the sibling, with every changed debate re-scored in the same run');
SELECT pg_temp.claims('A1', 'C1');
SELECT trending.refresh_agenda_listings(false);
SELECT assert((pg_temp.boost('A1')).rank_on_claim = 1
              AND pg_temp.trend('A2') = pg_temp.v('A2') - pg_temp.steps(pg_temp.drop_steps(), 1, 0),
              '4: restoring it moves the lift back');

-- ---------------------------------------------------------------------------
-- 5. Off again.
-- ---------------------------------------------------------------------------
UPDATE entity_ranking_config SET agenda_claim_steps = 0, agenda_follower_drop_steps = 0 WHERE id;
SELECT assert(trending.refresh_agenda_listings(false) >= 9, '5: steps back to 0: a non-forced hourly run re-scores the boosted debates');
SELECT assert(NOT EXISTS (SELECT 1 FROM trending.agenda_boosts), '5: and empties the table');
SELECT public.refresh_entity_ranking_scores(ARRAY(SELECT id FROM ids));
SELECT pg_temp.snapshot('off_again');
SELECT assert(pg_temp.same('zero', 'off_again'), '5: and gives back exactly the zero-config scores');
UPDATE entity_ranking_config SET agenda_claim_steps = 30, agenda_follower_drop_steps = 40 WHERE id;
SELECT pg_temp.full_pass();
SELECT assert(EXISTS (SELECT 1 FROM trending.agenda_boosts), '5: (on again)');
UPDATE entity_ranking_config SET ranking_places_mode = 1 WHERE id;
SELECT pg_temp.settle('rollback');
SELECT assert(pg_temp.same('shadow', 'rollback') AND NOT EXISTS (SELECT 1 FROM trending.agenda_boosts),
              '5: rolling back to mode 1 gives back exactly the shadow scores and empties the table');

-- Cleanup for running by hand.
UPDATE entity_ranking_config SET ranking_places_mode = 0, ranking_pushdown_since = 0, agenda_claim_steps = 0,
       agenda_follower_drop_steps = 0, agenda_day_factor = 0.65 WHERE id;
UPDATE trending.refresh_state SET last_full_pass_on = NULL WHERE id;
TRUNCATE entities, values, relations, votes_count, user_votes, spaces, entity_ranking_scores, entity_type_ranking,
         entity_topic_ranking, entity_feed_blocklist, trending.agenda_listings, trending.agenda_debate_positions,
         trending.ranking_place_sources, trending.ranking_places, trending.ranking_pushdowns,
         trending.ranking_shadow_report, trending.ranking_shadow_days, trending.agenda_boosts, ranks.rankings,
         ranks.ranking_items, ranks.ranking_scores, ranks.members, ranks.editors CASCADE;
DROP TABLE ids;
DROP TABLE snaps;
DROP TABLE base_boosts;
DROP TABLE extra_listed;
