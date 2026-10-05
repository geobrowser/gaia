-- Assertions for 0102: For you serving and feed experiments (GEO-3140, GEO-3144).
--
-- Run per drizzle/tests/README.md; also runs in CI via rankingSqlSuites.test.ts.
--
-- Candidates, passed in the order c3, c1, c2, c4 (not id order, to prove order is kept):
--   c1  scored 10, tagged t_a and t_b
--   c2  scored 9, tagged t_b; the user holds an Agree on it              -> excluded 'voted'
--   c3  scored 8, untagged; the user's Agree was taken back (type 2)     -> not excluded
--   c4  unscored, tagged t_a; the user marked it Interested             -> excluded 'interested'
-- plus a curation upvote on c1, which is not a position and does not exclude it.

\set ON_ERROR_STOP on

CREATE OR REPLACE FUNCTION assert(cond boolean, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
  IF cond IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: % (condition was %)', label, COALESCE(cond::text, 'NULL');
  ELSE RAISE NOTICE 'pass: %', label; END IF;
END $$;

CREATE TEMP TABLE ids AS SELECT * FROM (VALUES
  ('user',  '01020000-0000-4000-8000-000000000001'::uuid),
  ('other', '01020000-0000-4000-8000-000000000002'::uuid),
  ('c1',    '01020000-0000-4000-8000-0000000000c1'::uuid),
  ('c2',    '01020000-0000-4000-8000-0000000000c2'::uuid),
  ('c3',    '01020000-0000-4000-8000-0000000000c3'::uuid),
  ('c4',    '01020000-0000-4000-8000-0000000000c4'::uuid),
  ('t_a',   '01020000-0000-4000-8000-0000000000a1'::uuid),
  ('t_b',   '01020000-0000-4000-8000-0000000000b1'::uuid),
  ('space', '01020000-0000-4000-8000-0000000000f1'::uuid)
) AS v(key, id);
CREATE OR REPLACE FUNCTION pg_temp.id(k text) RETURNS uuid LANGUAGE sql AS $$ SELECT id FROM ids WHERE key = k $$;

DELETE FROM relations WHERE from_entity_id IN (SELECT id FROM ids);
DELETE FROM entity_ranking_scores WHERE entity_id IN (SELECT id FROM ids);
DELETE FROM user_votes WHERE user_id IN (SELECT id FROM ids);
DELETE FROM personalization.external_interest_signals WHERE user_id IN (SELECT id FROM ids);
DELETE FROM entities WHERE id IN (SELECT id FROM ids);

INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
SELECT id, '1790000000', '0', '1790000000', '0' FROM ids;

INSERT INTO entity_ranking_scores (entity_id, quality_score, intrinsic_score, participation_score, ranking_score,
                                   positive, negative, stance_positive, stance_negative, type_weight, updated_at)
SELECT pg_temp.id(k), 0.5, 0, 0, s, 0, 0, 0, 0, 1, now()
FROM (VALUES ('c1', 10.0), ('c2', 9.0), ('c3', 8.0)) AS v(k, s);

INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
SELECT gen_random_uuid(), gen_random_uuid(), '806d52bc-27e9-4c91-93c0-57978b093351'::uuid,
       pg_temp.id(f), pg_temp.id(t), pg_temp.id('space'), false
FROM (VALUES ('c1', 't_a'), ('c1', 't_b'), ('c2', 't_b'), ('c4', 't_a')) AS v(f, t);
-- A relation of another type is not a topic.
INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id, is_system)
VALUES (gen_random_uuid(), gen_random_uuid(), '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid,
        pg_temp.id('c3'), pg_temp.id('t_a'), pg_temp.id('space'), false);

INSERT INTO user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at) VALUES
  (pg_temp.id('user'),  pg_temp.id('c2'), 0, pg_temp.id('space'), 0, 1, now()),
  (pg_temp.id('user'),  pg_temp.id('c3'), 0, pg_temp.id('space'), 2, 1, now()),
  (pg_temp.id('user'),  pg_temp.id('c1'), 0, pg_temp.id('space'), 0, 0, now()),
  (pg_temp.id('other'), pg_temp.id('c1'), 0, pg_temp.id('space'), 0, 1, now());
INSERT INTO personalization.external_interest_signals (user_id, object_id, kind, occurred_at)
VALUES (pg_temp.id('user'), pg_temp.id('c4'), 'interested', now());

CREATE TEMP TABLE got AS
SELECT row_number() OVER () AS n, c.*
FROM personalization.for_you_candidates(pg_temp.id('user'),
       ARRAY[pg_temp.id('c3'), pg_temp.id('c1'), pg_temp.id('c2'), pg_temp.id('c4')]) c;

SELECT assert((SELECT array_agg(entity_id ORDER BY n) FROM got)
                = ARRAY[pg_temp.id('c3'), pg_temp.id('c1'), pg_temp.id('c2'), pg_temp.id('c4')],
  'candidates come back in the order given');
SELECT assert((SELECT ranking_score FROM got WHERE entity_id = pg_temp.id('c1')) = 10, 'Best score is carried');
SELECT assert((SELECT ranking_score IS NULL FROM got WHERE entity_id = pg_temp.id('c4')), 'an unscored candidate has a NULL score');
SELECT assert((SELECT topic_ids FROM got WHERE entity_id = pg_temp.id('c1')) = ARRAY[pg_temp.id('t_a'), pg_temp.id('t_b')],
  'topics are read from the Topics relation');
SELECT assert((SELECT topic_ids FROM got WHERE entity_id = pg_temp.id('c3')) = '{}', 'other relation types are not topics');
SELECT assert((SELECT excluded FROM got WHERE entity_id = pg_temp.id('c2')) = 'voted', 'a held position excludes the claim');
SELECT assert((SELECT excluded IS NULL FROM got WHERE entity_id = pg_temp.id('c3')), 'a retracted position does not');
SELECT assert((SELECT excluded IS NULL FROM got WHERE entity_id = pg_temp.id('c1')),
  'a curation vote, or another user''s position, does not');
SELECT assert((SELECT excluded FROM got WHERE entity_id = pg_temp.id('c4')) = 'interested', 'Interested on a question excludes it');

-- Revision: every tunable that shapes the page bumps it, statement by statement.
CREATE TEMP TABLE rev AS SELECT revision AS r0 FROM personalization.feed_config_revision;
UPDATE personalization.for_you_config SET exploration_share = exploration_share;
SELECT assert((SELECT revision FROM personalization.feed_config_revision) = (SELECT r0 + 1 FROM rev), 'for_you_config bumps the revision');
UPDATE personalization.interest_signal_weights SET weight = weight WHERE kind = 'vote';
SELECT assert((SELECT revision FROM personalization.feed_config_revision) = (SELECT r0 + 2 FROM rev), 'interest weights bump the revision');
UPDATE personalization.interest_config SET half_life_days = half_life_days;
SELECT assert((SELECT revision FROM personalization.feed_config_revision) = (SELECT r0 + 3 FROM rev), 'interest half-life bumps the revision');
UPDATE entity_ranking_config SET tau_seconds = tau_seconds;
SELECT assert((SELECT revision FROM personalization.feed_config_revision) = (SELECT r0 + 4 FROM rev), 'Best''s ranking config bumps the revision');
SELECT assert((SELECT changed_table FROM personalization.feed_config_revision) = 'public.entity_ranking_config', 'and says which table');

-- Experiments.
DELETE FROM personalization.feed_experiments;
SELECT assert(NOT EXISTS (SELECT 1 FROM personalization.feed_experiment_for_user(pg_temp.id('user'))), 'no active experiment, no row');

INSERT INTO personalization.feed_experiments (id, active, arm_a, arm_b, share, salt)
VALUES ('aa-check', true, 'best', 'best', 0, 's1');
SELECT assert((SELECT NOT interleaved AND assignment = 'hash' FROM personalization.feed_experiment_for_user(pg_temp.id('user'))),
  'share 0 puts nobody in by hash');
UPDATE personalization.feed_experiments SET share = 1;
SELECT assert((SELECT interleaved FROM personalization.feed_experiment_for_user(pg_temp.id('user'))), 'share 1 puts everyone in');
INSERT INTO personalization.feed_experiment_members (experiment_id, user_id, interleaved) VALUES ('aa-check', pg_temp.id('user'), false);
SELECT assert((SELECT NOT interleaved AND assignment = 'member' FROM personalization.feed_experiment_for_user(pg_temp.id('user'))),
  'explicit membership wins over the hash');

-- Half the users at share 0.5, deterministically.
UPDATE personalization.feed_experiments SET share = 0.5;
CREATE TEMP TABLE split AS
SELECT g, (SELECT interleaved FROM personalization.feed_experiment_for_user(md5('u' || g)::uuid)) AS i
FROM generate_series(1, 2000) g;
SELECT assert((SELECT count(*) FILTER (WHERE i) BETWEEN 900 AND 1100 FROM split), 'share 0.5 puts about half in');
SELECT assert((SELECT bool_and(s.i = (SELECT interleaved FROM personalization.feed_experiment_for_user(md5('u' || s.g)::uuid))) FROM split s),
  'assignment is stable across calls');

DO $$ BEGIN
  INSERT INTO personalization.feed_experiments (id, active, arm_a, arm_b) VALUES ('second', true, 'best', 'for-you');
  RAISE EXCEPTION 'a second active experiment was accepted';
EXCEPTION WHEN unique_violation THEN RAISE NOTICE 'pass: only one experiment is active';
END $$;
DO $$ BEGIN
  INSERT INTO personalization.feed_experiments (id, arm_a, arm_b) VALUES ('bad', 'best', 'top');
  RAISE EXCEPTION 'an unknown arm was accepted';
EXCEPTION WHEN check_violation THEN RAISE NOTICE 'pass: arms must be feeds the app can build';
END $$;

DELETE FROM personalization.feed_experiments;
DELETE FROM relations WHERE from_entity_id IN (SELECT id FROM ids);
DELETE FROM entity_ranking_scores WHERE entity_id IN (SELECT id FROM ids);
DELETE FROM user_votes WHERE user_id IN (SELECT id FROM ids);
DELETE FROM personalization.external_interest_signals WHERE user_id IN (SELECT id FROM ids);
DELETE FROM entities WHERE id IN (SELECT id FROM ids);
