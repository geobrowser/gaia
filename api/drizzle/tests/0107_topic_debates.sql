-- Assertions for 0107: Best ranks topic debates (GEO-3150), and topic debates credit their topic in
-- the interest model (GEO-3088 "Topic debates").
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
--   1. Every topic debate gets a score, and lands in Debate's type ranking and its topic's ranking.
--   2. More debates already held on the topic: higher than an otherwise equal topic debate.
--   3. More Interested on the topic: higher. Each person once; removals, weight-0 accounts and the
--      old Following relation do not count; a down-weighted account counts for less.
--   4. The term is added to the ordinary score, so recency works as for any debate; it is capped.
--   5. Never a main claim: a Debate with a Claims relation is a claim debate, whatever else it has.
--   6. Claim debates (and everything else) are unchanged: same scores after every topic input
--      moves, and still exactly 0098's expression.
--   7. refresh_topic_debate_scores picks up a new Interested; nothing else does.
--   8. entitiesInBestOrder orders topic and claim debates together, unscored last.
--   9. Interest model: a topic debate credits its topic at the debate weight, and no follow; a
--      claim debate still credits its claim's topics only.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

TRUNCATE entities, values, relations, votes_count, user_votes, spaces, entity_ranking_scores,
         entity_type_weights, entity_type_exclusions, entity_type_ranking, entity_topic_ranking,
         entity_feed_blocklist, account_weights, account_exclusions,
         personalization.external_interest_signals CASCADE;

UPDATE entity_ranking_config
   SET participation_weight = 4, participation_cap = 19, comment_weight = 0, comment_cap = 30,
       tau_seconds = 100000, topic_interested_weight = 0.5, topic_debate_weight = 0.5, topic_cap = 2.0
 WHERE id;

-- ---------------------------------------------------------------------------
-- Fixtures.
--   Topics TA, TB, TC, TD, TK (TK is the claim k1's topic).
--   H1, H2   claim debates on k1, filed under TA, created before the topic debates.
--   CD       a claim debate on k1, filed under TA, with votes; P2 took part.
--   DX       a Debate with BOTH Claims (k1) and Topics (TA): a claim debate. Created after DA.
--   DN       a Debate with neither: not a topic debate.
--   DA, DB, DC, DH   topic debates on TA, TB, TC, TD, all created at the same moment with the
--            same shape (Types, Topics, one Participant, a Name), no votes. P1 took part in DA.
--   k1       a claim tagged TK, with votes.
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE ids AS SELECT * FROM (VALUES
  ('TA', 'b1060000-0000-4000-8000-000000000101'::uuid, NULL::bigint),
  ('TB', 'b1060000-0000-4000-8000-000000000102'::uuid, NULL),
  ('TC', 'b1060000-0000-4000-8000-000000000103'::uuid, NULL),
  ('TD', 'b1060000-0000-4000-8000-000000000104'::uuid, NULL),
  ('TK', 'b1060000-0000-4000-8000-000000000105'::uuid, NULL),
  ('k1', 'b1060000-0000-4000-8000-000000000201'::uuid, 1790000000),
  ('H1', 'b1060000-0000-4000-8000-000000000301'::uuid, 1790000000),
  ('H2', 'b1060000-0000-4000-8000-000000000302'::uuid, 1790000100),
  ('CD', 'b1060000-0000-4000-8000-000000000303'::uuid, 1790300000),
  ('DX', 'b1060000-0000-4000-8000-000000000304'::uuid, 1790500000),
  ('DN', 'b1060000-0000-4000-8000-000000000305'::uuid, 1790400000),
  ('DA', 'b1060000-0000-4000-8000-000000000401'::uuid, 1790400000),
  ('DB', 'b1060000-0000-4000-8000-000000000402'::uuid, 1790400000),
  ('DC', 'b1060000-0000-4000-8000-000000000403'::uuid, 1790400000),
  ('DH', 'b1060000-0000-4000-8000-000000000404'::uuid, 1790400000),
  -- people
  ('P1', 'b1060000-0000-4000-8000-000000000501'::uuid, NULL),
  ('P2', 'b1060000-0000-4000-8000-000000000502'::uuid, NULL),
  ('U1', 'b1060000-0000-4000-8000-000000000601'::uuid, NULL),
  ('U2', 'b1060000-0000-4000-8000-000000000602'::uuid, NULL),
  ('U0', 'b1060000-0000-4000-8000-000000000603'::uuid, NULL),
  ('UR', 'b1060000-0000-4000-8000-000000000604'::uuid, NULL),
  ('UF', 'b1060000-0000-4000-8000-000000000605'::uuid, NULL),
  ('UH', 'b1060000-0000-4000-8000-000000000606'::uuid, NULL),
  ('S',  'b1060000-0000-4000-8000-0000000000d0'::uuid, NULL),
  ('S2', 'b1060000-0000-4000-8000-0000000000d1'::uuid, NULL),
  -- real ids
  ('DEBATE', 'fd51f935-2063-4617-be39-7b672b23364c'::uuid, NULL),
  ('CLAIM',  '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid, NULL),
  ('TOPIC',  '5ef5a586-0f27-4d8e-8f6c-59ae5b3e89e2'::uuid, NULL)
) AS v(k, id, created);
CREATE OR REPLACE FUNCTION pg_temp.i(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM ids WHERE ids.k = $1 $$;

CREATE OR REPLACE FUNCTION pg_temp.rel(from_k text, type_id uuid, to_k text, space_k text DEFAULT 'S') RETURNS void
LANGUAGE sql AS $$
  INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
  VALUES (gen_random_uuid(), gen_random_uuid(), type_id, pg_temp.i(from_k), pg_temp.i(to_k), pg_temp.i(space_k), false)
$$;
CREATE OR REPLACE FUNCTION pg_temp.interested(who text, topic text, vote_type smallint, space_k text DEFAULT 'S') RETURNS void
LANGUAGE sql AS $$
  INSERT INTO user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at)
  VALUES (pg_temp.i(who), pg_temp.i(topic), 0, pg_temp.i(space_k), vote_type, 3, now())
$$;
CREATE OR REPLACE FUNCTION pg_temp.score(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT ranking_score FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k) $$;
CREATE OR REPLACE FUNCTION pg_temp.topic(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT topic_score FROM entity_ranking_scores WHERE entity_id = pg_temp.i(k) $$;
-- 0098's expression for a stored row: what ranking_score must equal for anything not a topic debate.
CREATE OR REPLACE FUNCTION pg_temp.base(k text) RETURNS numeric LANGUAGE sql AS $$
  SELECT public.entity_ranking_score(s.quality_score, s.type_weight, s.intrinsic_score,
                                     s.participation_score, s.comment_score,
                                     NULLIF(e.created_at, '')::bigint, c.tau_seconds, c.quality_floor)
  FROM entity_ranking_scores s JOIN entities e ON e.id = s.entity_id CROSS JOIN entity_ranking_config c
  WHERE s.entity_id = pg_temp.i(k) AND c.id $$;
CREATE OR REPLACE FUNCTION pg_temp.near(a numeric, b numeric) RETURNS boolean LANGUAGE sql AS $$
  SELECT abs(a - b) < 1e-9 $$;

INSERT INTO spaces (id, type, address)
SELECT id, 'Personal', 'addr-' || k FROM ids WHERE k IN ('P1','P2','U1','U2','U0','UR','UF','UH');

INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
SELECT id, coalesce(created, 1780000000)::text, '0', coalesce(created, 1780000000)::text, '0'
FROM ids WHERE k NOT IN ('DEBATE','CLAIM','TOPIC','S','S2');

INSERT INTO values (id, entity_id, property_id, space_id, text)
SELECT gen_random_uuid()::text, id, 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid, pg_temp.i('S'), 'fixture ' || k
FROM ids WHERE created IS NOT NULL;

DO $$
DECLARE
  types     constant uuid := '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1';
  topics    constant uuid := '806d52bc-27e9-4c91-93c0-57978b093351';
  claims    constant uuid := 'e614cce1-c4ce-4586-8304-fd1237119eb2';
  parts     constant uuid := '0b9b1a35-2068-4431-8f7d-2350f958a728';
  following constant uuid := 'f374b8f2-d331-48a3-a220-ba3648992e93';
  t text;
BEGIN
  FOREACH t IN ARRAY ARRAY['TA','TB','TC','TD','TK'] LOOP PERFORM pg_temp.rel(t, types, 'TOPIC'); END LOOP;
  PERFORM pg_temp.rel('k1', types, 'CLAIM');
  PERFORM pg_temp.rel('k1', topics, 'TK');
  -- Claim debates: typed, one Claims relation, filed under TA as the web app files them.
  FOREACH t IN ARRAY ARRAY['H1','H2','CD','DX'] LOOP
    PERFORM pg_temp.rel(t, types, 'DEBATE');
    PERFORM pg_temp.rel(t, claims, 'k1');
    PERFORM pg_temp.rel(t, topics, 'TA');
  END LOOP;
  PERFORM pg_temp.rel('CD', parts, 'P2');
  PERFORM pg_temp.rel('DN', types, 'DEBATE');
  -- Topic debates: same shape each.
  PERFORM pg_temp.rel('DA', types, 'DEBATE'); PERFORM pg_temp.rel('DA', topics, 'TA'); PERFORM pg_temp.rel('DA', parts, 'P1');
  PERFORM pg_temp.rel('DB', types, 'DEBATE'); PERFORM pg_temp.rel('DB', topics, 'TB'); PERFORM pg_temp.rel('DB', parts, 'P1');
  PERFORM pg_temp.rel('DC', types, 'DEBATE'); PERFORM pg_temp.rel('DC', topics, 'TC'); PERFORM pg_temp.rel('DC', parts, 'P1');
  PERFORM pg_temp.rel('DH', types, 'DEBATE'); PERFORM pg_temp.rel('DH', topics, 'TD'); PERFORM pg_temp.rel('DH', parts, 'P1');
  -- The old Following relation, in the follower's own space: dropped, must not count.
  PERFORM pg_temp.rel('UF', following, 'TC', 'UF');
END $$;

INSERT INTO votes_count (object_id, object_type, space_id, vote_kind, positive, negative) VALUES
  (pg_temp.i('CD'), 0, pg_temp.i('S'), 1, 3, 1),
  (pg_temp.i('k1'), 0, pg_temp.i('S'), 0, 5, 2),
  (pg_temp.i('k1'), 0, pg_temp.i('S'), 1, 4, 4);

INSERT INTO account_weights (user_id, weight, reasons, vote_count, stance_vote_count, first_vote_at, computed_at)
SELECT pg_temp.i(k), w, '[]'::jsonb, 1, 0, now(), now()
FROM (VALUES ('U1', 1.0), ('U2', 1.0), ('U0', 0.0), ('UR', 1.0), ('UF', 1.0), ('UH', 0.5)) v(k, w);

-- First scoring: no Interested anywhere yet.
SELECT public.refresh_entity_ranking_scores(ARRAY(SELECT id FROM ids WHERE created IS NOT NULL));

-- Snapshot of everything that is not a topic debate, for criterion 6.
CREATE TEMP TABLE before_scores AS
SELECT k, s.* FROM ids JOIN entity_ranking_scores s ON s.entity_id = ids.id
WHERE k IN ('k1','H1','H2','CD','DX','DN');

-- ---------------------------------------------------------------------------
-- 1. Every topic debate has a score and is in Debate's and its topic's rankings.
-- ---------------------------------------------------------------------------
SELECT assert((SELECT count(*) FROM entity_ranking_scores
               WHERE entity_id IN (pg_temp.i('DA'), pg_temp.i('DB'), pg_temp.i('DC'), pg_temp.i('DH'))) = 4,
              '1: every topic debate has a Best score');
SELECT assert(EXISTS (SELECT 1 FROM entity_type_ranking
                      WHERE type_id = pg_temp.i('DEBATE') AND entity_id = pg_temp.i('DA')
                        AND ranking_score = pg_temp.score('DA')),
              '1: a topic debate ranks among Debates (Explore''s debate slot and type mix)');
SELECT assert(EXISTS (SELECT 1 FROM entity_topic_ranking
                      WHERE topic_id = pg_temp.i('TA') AND type_id = pg_temp.i('DEBATE')
                        AND entity_id = pg_temp.i('DA') AND ranking_score = pg_temp.score('DA')),
              '1: a topic debate ranks under its topic (For you)');

-- ---------------------------------------------------------------------------
-- 2. Debates already held on the topic. TA has H1, H2 and CD before DA; DX is later.
-- ---------------------------------------------------------------------------
SELECT assert((SELECT debates_held FROM public.topic_debate_scores(ARRAY[pg_temp.i('DA')])) = 3,
              '2: debates held counts earlier debates on the topic, claim debates included, not later ones');
SELECT assert(pg_temp.topic('DB') = 0, '2: no earlier debate and no Interested: no topic term');
SELECT assert(pg_temp.near(pg_temp.topic('DA'), 0.5 * ln(4::numeric)), '2: 0.5 x ln(1 + 3)');
SELECT assert(pg_temp.score('DA') > pg_temp.score('DB'),
              '2: more debates held on the topic scores higher than an otherwise equal topic debate');
SELECT assert(pg_temp.near(pg_temp.score('DA') - pg_temp.score('DB'), pg_temp.topic('DA')),
              '2: and by exactly the topic term (the two are otherwise equal)');

-- ---------------------------------------------------------------------------
-- 3. Interested. On TC: U1 (weight 1), U2 (weight 1, in two spaces), U0 (weight 0), UR (cleared),
--    plus UF's Following relation. On TD: UH (weight 0.5).
-- ---------------------------------------------------------------------------
SELECT pg_temp.interested('U1', 'TC', 0::smallint);
SELECT pg_temp.interested('U2', 'TC', 0::smallint, 'S');
SELECT pg_temp.interested('U2', 'TC', 0::smallint, 'S2');
SELECT pg_temp.interested('U0', 'TC', 0::smallint);
SELECT pg_temp.interested('UR', 'TC', 2::smallint);
SELECT pg_temp.interested('UH', 'TD', 0::smallint);
SELECT public.refresh_topic_debate_scores();

SELECT assert((SELECT interested FROM public.topic_debate_scores(ARRAY[pg_temp.i('DC')])) = 2.0,
              '3: Interested on TC = 2.0: each person once, weight 0 and cleared do not count, Following does not count');
SELECT assert(pg_temp.near(pg_temp.topic('DC'), 0.5 * ln(3::numeric)), '3: 0.5 x ln(1 + 2)');
SELECT assert(pg_temp.score('DC') > pg_temp.score('DB'),
              '3: more Interested on the topic scores higher than an otherwise equal topic debate');
SELECT assert((SELECT interested FROM public.topic_debate_scores(ARRAY[pg_temp.i('DH')])) = 0.5,
              '3: a down-weighted account (0.5) counts for half');
SELECT assert(pg_temp.topic('DH') < pg_temp.topic('DC') AND pg_temp.topic('DH') > 0,
              '3: so it lifts less than full-weight Interested, but still lifts');

-- ---------------------------------------------------------------------------
-- 4. Added to the ordinary score, so recency is the same as any debate's; capped.
-- ---------------------------------------------------------------------------
SELECT assert(bool_and(pg_temp.near(pg_temp.score(k), pg_temp.base(k) + pg_temp.topic(k))),
              '4: a topic debate''s score is the ordinary score (recency included) plus the topic term')
FROM unnest(ARRAY['DA','DB','DC','DH']) k;
UPDATE entity_ranking_config SET topic_debate_weight = 100 WHERE id;
SELECT public.refresh_topic_debate_scores();
SELECT assert(pg_temp.topic('DA') = 2.0, '4: the term is capped at topic_cap');
UPDATE entity_ranking_config SET topic_debate_weight = 0.5 WHERE id;
SELECT public.refresh_topic_debate_scores();
SELECT assert(pg_temp.near(pg_temp.topic('DA'), 0.5 * ln(4::numeric)), '4: and comes back when the weight does');

-- ---------------------------------------------------------------------------
-- 5. Never a main claim. DX has Topics and Claims; DN has neither; claim debates have Claims.
-- ---------------------------------------------------------------------------
SELECT assert(NOT EXISTS (SELECT 1 FROM public.topic_debate_scores(ARRAY(
                SELECT pg_temp.i(k) FROM unnest(ARRAY['H1','H2','CD','DX','DN','k1']) k))),
              '5: claim debates (even filed under a topic), untopiced debates and claims are not topic debates');
SELECT assert((SELECT array_agg(entity_id ORDER BY entity_id) FROM public.topic_debate_scores(ARRAY(SELECT id FROM ids)))
              = ARRAY[pg_temp.i('DA'), pg_temp.i('DB'), pg_temp.i('DC'), pg_temp.i('DH')],
              '5: exactly the four topic debates are topic debates');

-- ---------------------------------------------------------------------------
-- 6. Claim debates and everything else unchanged, after every topic input moves: Interested on TA
--    (their own topic) from three people, another debate on TA, and much larger weights.
-- ---------------------------------------------------------------------------
SELECT pg_temp.interested(k, 'TA', 0::smallint) FROM unnest(ARRAY['U1','U2','UH']) k;
UPDATE entity_ranking_config SET topic_interested_weight = 5, topic_debate_weight = 5, topic_cap = 10 WHERE id;
SELECT public.refresh_entity_ranking_scores(ARRAY(SELECT id FROM ids WHERE created IS NOT NULL));
SELECT assert(bool_and(s.ranking_score = b.ranking_score AND s.topic_score = 0
                       AND s.participation_score = b.participation_score
                       AND s.intrinsic_score = b.intrinsic_score AND s.quality_score = b.quality_score),
              '6: claim debates, untopiced debates and claims score exactly as before ('
              || count(*) || ' rows)')
FROM before_scores b JOIN entity_ranking_scores s ON s.entity_id = b.entity_id;
SELECT assert((SELECT count(*) FROM before_scores) = 6, '6: and the comparison covered all six');
SELECT assert(bool_and(pg_temp.score(k) = pg_temp.base(k)),
              '6: and each is still exactly 0098''s expression, with nothing added')
FROM unnest(ARRAY['k1','H1','H2','CD','DX','DN']) k;
SELECT assert(pg_temp.topic('DA') > 0.5 * ln(4::numeric),
              '6: (control) the same inputs did move the topic debate on TA');
UPDATE entity_ranking_config SET topic_interested_weight = 0.5, topic_debate_weight = 0.5, topic_cap = 2.0 WHERE id;
SELECT public.refresh_topic_debate_scores();

-- ---------------------------------------------------------------------------
-- 7. Freshness: a new Interested on TB moves DB only once refresh_topic_debate_scores runs.
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE db_before AS SELECT pg_temp.score('DB') AS s;
SELECT pg_temp.interested('U1', 'TB', 0::smallint);
SELECT assert(pg_temp.score('DB') = (SELECT s FROM db_before), '7: an Interested alone re-scores nothing');
SELECT assert(public.refresh_topic_debate_scores() = 4, '7: refresh_topic_debate_scores re-scores the four topic debates');
SELECT assert(pg_temp.score('DB') > (SELECT s FROM db_before), '7: and the new Interested reaches the score');

-- ---------------------------------------------------------------------------
-- 8. entitiesInBestOrder (0104) orders topic and claim debates together; unscored last.
-- ---------------------------------------------------------------------------
INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
VALUES ('b1060000-0000-4000-8000-0000000009f9', '1790400000', '0', '1790400000', '0');
SELECT assert(
  (SELECT array_agg(e.id) FROM public.entities_in_best_order(ARRAY[
     'b1060000-0000-4000-8000-0000000009f9'::uuid, pg_temp.i('DB'), pg_temp.i('CD'), pg_temp.i('DA')]) e)
  = (SELECT array_agg(x.id ORDER BY x.s DESC, x.id DESC) FROM (
       SELECT pg_temp.i(k) AS id, pg_temp.score(k) AS s FROM unnest(ARRAY['DB','CD','DA']) k) x)
    || 'b1060000-0000-4000-8000-0000000009f9'::uuid,
  '8: debates of both kinds in Best''s order, the unscored one last');

-- ---------------------------------------------------------------------------
-- 9. Interest model. P1 took part in DA (topic debate on TA); P2 in CD (claim debate on k1, which
--    is about TK, the debate itself filed under TA).
-- ---------------------------------------------------------------------------
CREATE TEMP TABLE sig AS
SELECT * FROM personalization.compute_user_topic_signals(ARRAY[pg_temp.i('P1'), pg_temp.i('P2')], now());
SELECT assert(EXISTS (SELECT 1 FROM sig WHERE user_id = pg_temp.i('P1') AND topic_id = pg_temp.i('TA')
                        AND kind = 'debate' AND top_object_id = pg_temp.i('DA')),
              '9: joining a topic debate credits its topic as a debate');
SELECT assert((SELECT w.weight FROM personalization.interest_signal_weights w WHERE w.kind = 'debate') = 2.0,
              '9: at the same weight as a claim debate (one kind, 2.0)');
SELECT assert((SELECT count(DISTINCT topic_id) FROM sig WHERE user_id = pg_temp.i('P1') AND kind = 'debate') = 4,
              '9: each topic debate credits its own topic (TA, TB, TC, TD)');
SELECT assert(NOT EXISTS (SELECT 1 FROM sig WHERE user_id = pg_temp.i('P1') AND kind <> 'debate'),
              '9: and nothing else: joining adds no follow credit');
SELECT assert(EXISTS (SELECT 1 FROM sig WHERE user_id = pg_temp.i('P2') AND topic_id = pg_temp.i('TK') AND kind = 'debate')
              AND NOT EXISTS (SELECT 1 FROM sig WHERE user_id = pg_temp.i('P2') AND topic_id = pg_temp.i('TA')),
              '9: a claim debate still credits its claim''s topics, not the debate''s own Topics');

-- Clean up for manual psql runs.
TRUNCATE entities, values, relations, votes_count, user_votes, spaces, entity_ranking_scores,
         entity_type_ranking, entity_topic_ranking, account_weights CASCADE;
