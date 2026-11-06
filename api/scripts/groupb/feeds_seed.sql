-- Seeds one random graph for the feed and primer reads. :seed is an integer; setseed makes it reproducible.
SELECT set_config('gb.seed', :'seed', false) \gset
SELECT setseed((:seed)::float8 / 1000.0) \gset
TRUNCATE public.entities, public.spaces, public.relations, public."values", public.entity_ranking_scores, public.entity_feed_blocklist,
  public.entity_type_exclusions, public.entity_type_ranking, public.entity_topic_ranking, public.user_votes,
  public.primer_claim_stats, public.anchor_claim_members;
TRUNCATE public.anchor_claim_sets RESTART IDENTITY;

CREATE SEQUENCE pg_temp.ids;
CREATE FUNCTION pg_temp.u(kind text, i int) RETURNS uuid LANGUAGE sql IMMUTABLE AS
$$ SELECT md5(current_setting('gb.seed') || kind || i::text)::uuid $$;
CREATE FUNCTION pg_temp.pick(kind text, n int) RETURNS uuid LANGUAGE sql VOLATILE AS
$$ SELECT pg_temp.u(kind, 1 + floor(random() * n)::int) $$;

-- spaces sp1..5 (sp5 has no topic), topics topic1..6, types t1..6 (t6 is excluded), entities e1..60
INSERT INTO public.spaces SELECT pg_temp.u('sp', i), 'DAO', CASE WHEN i <= 4 THEN pg_temp.u('topic', i) END FROM generate_series(1, 5) i;
INSERT INTO public.entities
SELECT pg_temp.u('e', i), CASE WHEN random() < 0.06 THEN 'x' ELSE (1760000000 + floor(random() * 2500000)::int)::text END
FROM generate_series(1, 60) i;
INSERT INTO public.entity_type_exclusions VALUES (pg_temp.u('t', 6));

-- Names: most entities have one in a random space, a few a blank or null one, a few a second one elsewhere.
INSERT INTO public."values"
SELECT md5(current_setting('gb.seed') || 'v' || nextval('pg_temp.ids'))::uuid::text, 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid,
       pg_temp.u('e', i), pg_temp.pick('sp', 5),
       CASE WHEN random() < 0.08 THEN '   ' WHEN random() < 0.08 THEN NULL WHEN random() < 0.05 THEN '' WHEN random() < 0.06 THEN E'\t' ELSE 'name ' || i END
FROM generate_series(1, 60) i WHERE random() < 0.9;
INSERT INTO public."values"
SELECT md5(current_setting('gb.seed') || 'v' || nextval('pg_temp.ids'))::uuid::text, 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid,
       pg_temp.u('e', i), pg_temp.pick('sp', 5), 'second ' || i
FROM generate_series(1, 60) i WHERE random() < 0.25;

-- Types: each entity gets 1-2 of Claim, Debate, t1..t6; ~6% are System Type. A third are claims for the primer.
CREATE TABLE pg_temp.ty AS SELECT * FROM (VALUES (1, '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid), (2, '55c95b26-26f8-482c-b973-9ea99dfde438'::uuid),
  (3, pg_temp.u('t', 1)), (4, pg_temp.u('t', 2)), (5, pg_temp.u('t', 3)), (6, pg_temp.u('t', 4)), (7, pg_temp.u('t', 5)), (8, pg_temp.u('t', 6))) v(n, id);
CREATE FUNCTION pg_temp.tyid(k int) RETURNS uuid LANGUAGE sql STABLE AS $$ SELECT id FROM pg_temp.ty WHERE n = k $$;
INSERT INTO public.relations
SELECT md5(current_setting('gb.seed') || 'r' || nextval('pg_temp.ids'))::uuid, '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid,
       pg_temp.u('e', i), pg_temp.tyid(CASE WHEN random() < 0.3 THEN 1 WHEN random() < 0.2 THEN 2 ELSE 1 + floor(random() * 8)::int END), NULL, pg_temp.pick('sp', 5)
FROM generate_series(1, 60) i, generate_series(1, 2) k WHERE k = 1 OR random() < 0.4;
INSERT INTO public.relations
SELECT md5(current_setting('gb.seed') || 'r' || nextval('pg_temp.ids'))::uuid, '88b3d6ad-288c-529c-a212-0e1c24819185'::uuid,
       pg_temp.u('e', i), pg_temp.u('t', 1), NULL, pg_temp.pick('sp', 5)
FROM generate_series(1, 60) i WHERE random() < 0.06;
-- debate-tag relations: tagged with the Debate type in a random space
INSERT INTO public.relations
SELECT md5(current_setting('gb.seed') || 'r' || nextval('pg_temp.ids'))::uuid, '25709034-1ba5-406f-94e4-d4af90042fba'::uuid,
       pg_temp.u('e', i), '55c95b26-26f8-482c-b973-9ea99dfde438'::uuid, NULL, pg_temp.pick('sp', 5)
FROM generate_series(1, 60) i WHERE random() < 0.35;
-- the same Types relation twice in different spaces for a few entities
INSERT INTO public.relations
SELECT md5(current_setting('gb.seed') || 'r' || nextval('pg_temp.ids'))::uuid, type_id, from_entity_id, to_entity_id, NULL, pg_temp.pick('sp', 5)
FROM public.relations WHERE type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid AND random() < 0.15;

INSERT INTO public.entity_ranking_scores SELECT pg_temp.u('e', i), floor(random() * 21) FROM generate_series(1, 60) i WHERE random() < 0.9;
INSERT INTO public.entity_feed_blocklist SELECT pg_temp.u('e', i) FROM generate_series(1, 60) i WHERE random() < 0.07;

-- Derived rankings: one row per Types relation, scored like the entity (or a stale random score), and per (topic, type) for 0-3 topics.
INSERT INTO public.entity_type_ranking
SELECT r.to_entity_id, r.from_entity_id, coalesce(max(s.ranking_score), floor(random() * 21))
FROM public.relations r LEFT JOIN public.entity_ranking_scores s ON s.entity_id = r.from_entity_id
WHERE r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid GROUP BY r.to_entity_id, r.from_entity_id;
CREATE TABLE pg_temp.et AS SELECT e.id,
  coalesce((SELECT ranking_score FROM public.entity_ranking_scores s WHERE s.entity_id = e.id), floor(random() * 21)) AS score,
  CASE WHEN random() < 0.1 THEN NULL ELSE (1760000000 + floor(random() * 2500000)::int)::bigint END AS epoch
FROM public.entities e;
INSERT INTO public.entity_topic_ranking
SELECT pg_temp.u('topic', 1 + floor(random() * 6)::int), r.to_entity_id, r.from_entity_id, et.score, et.epoch
FROM public.relations r JOIN pg_temp.et ON et.id = r.from_entity_id, generate_series(1, 3) g
WHERE r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid AND random() < 0.55
ON CONFLICT DO NOTHING;
-- a correlated topic assignment so match_all intersections are not empty: topic1 and topic2 both on a third of entities
INSERT INTO public.entity_topic_ranking
SELECT pg_temp.u('topic', k), r.to_entity_id, r.from_entity_id, et.score, et.epoch
FROM public.relations r JOIN pg_temp.et ON et.id = r.from_entity_id, generate_series(1, 2) k
WHERE r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid AND (get_byte(decode(md5(r.from_entity_id::text), 'hex'), 0) % 3) = 0
ON CONFLICT DO NOTHING;

-- Primer: stats for the claims (a Claim Types relation), votes by 4 users, anchor sets (one in the future).
CREATE TABLE pg_temp.cl AS
SELECT from_entity_id AS id, floor(random() * 22) / 2 AS w FROM public.relations
WHERE type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid AND to_entity_id = '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid GROUP BY from_entity_id;
INSERT INTO public.primer_claim_stats
SELECT c.id, ARRAY(SELECT DISTINCT pg_temp.pick('sp', 5) FROM generate_series(1, 1 + floor(random() * 3)::int + (c.id IS NULL)::int)),
       ARRAY(SELECT DISTINCT pg_temp.pick('topic', 8) FROM generate_series(1, floor(random() * 4)::int + (c.id IS NULL)::int)),
       floor(random() * 12), c.w, c.w / 2, 0.5, random(), round((random() * 5)::numeric, 3)::float8, random() < 0.5,
       round((random() * 10)::numeric, 3)::float8, 3 + floor(random() * 80)::int, now()
FROM pg_temp.cl c WHERE random() < 0.9;
INSERT INTO public.user_votes
SELECT pg_temp.pick('user', 4), p.claim_id, CASE WHEN random() < 0.8 THEN 1 ELSE 0 END
FROM public.primer_claim_stats p, generate_series(1, 2) k WHERE random() < 0.25;
INSERT INTO public.anchor_claim_sets (effective_from, claim_count, params) VALUES
  ('2026-01-01T00:00:00Z', 0, '{}'), ('2026-09-01T00:00:00Z', 0, '{}'), ('2099-01-01T00:00:00Z', 0, '{}');
INSERT INTO public.anchor_claim_members
SELECT sets.id, p.claim_id, row_number() OVER (PARTITION BY sets.id ORDER BY random()), random(), 3, 0.5
FROM public.primer_claim_stats p, public.anchor_claim_sets sets WHERE random() < 0.3;
