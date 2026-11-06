-- Seeds one random graph for the trending and topic-suggestion reads. :seed is an integer; setseed makes it reproducible.
-- trending.today() is pinned to 2026-11-06 by gen_golden.py.
SELECT set_config('gb.seed', :'seed', false) \gset
SELECT setseed((:seed)::float8 / 1000.0) \gset
TRUNCATE public.entities, public.spaces, public.relations, public."values", trending.agenda_listings, public.space_topic_suggestions;

CREATE SEQUENCE pg_temp.ids;
CREATE FUNCTION pg_temp.u(kind text, i int) RETURNS uuid LANGUAGE sql IMMUTABLE AS
$$ SELECT md5(current_setting('gb.seed') || kind || i::text)::uuid $$;
CREATE FUNCTION pg_temp.pick(kind text, n int) RETURNS uuid LANGUAGE sql VOLATILE AS
$$ SELECT pg_temp.u(kind, 1 + floor(random() * n)::int) $$;
CREATE FUNCTION pg_temp.sp() RETURNS uuid LANGUAGE sql VOLATILE AS $$ SELECT pg_temp.pick('sp', 5) $$;
CREATE FUNCTION pg_temp.dao() RETURNS uuid LANGUAGE sql VOLATILE AS $$ SELECT pg_temp.pick('sp', 3) $$;
CREATE FUNCTION pg_temp.pos() RETURNS text LANGUAGE sql VOLATILE AS
$$ SELECT CASE WHEN random() < 0.12 THEN NULL
  ELSE (SELECT string_agg(substr('aAbBcCzZ0189', 1 + floor(random() * 12)::int, 1), '') FROM generate_series(1, 1 + floor(random() * 2)::int)) END $$;
CREATE FUNCTION pg_temp.rel(typ uuid, f uuid, t uuid, sp uuid) RETURNS void LANGUAGE sql AS
$$ INSERT INTO public.relations VALUES (md5(current_setting('gb.seed') || 'r' || nextval('pg_temp.ids'))::uuid, typ, f, t, pg_temp.pos(), sp) $$;

INSERT INTO public.spaces SELECT pg_temp.u('sp', i), CASE WHEN i <= 3 THEN 'DAO' ELSE 'PERSONAL' END, pg_temp.u('topic', i)
FROM generate_series(1, 5) i;

-- claims c1..14, debates d1..10, topics topic1..8 (1..5 are the spaces' own), agendas a1..8, blocks b1..10, others o1..4
INSERT INTO public.entities
SELECT pg_temp.u(k, i), CASE WHEN random() < 0.1 THEN 'x' ELSE (1760000000 + floor(random() * 2500000)::int)::text END
FROM (VALUES ('c', 14), ('d', 10), ('topic', 8), ('a', 8), ('b', 10), ('o', 4)) v(k, n), generate_series(1, v.n) i;

-- Types: claims and debates in a random space, a few of both, topics, agendas (one space each so listings never tie across spaces).
SELECT pg_temp.rel('8f151ba4-de20-4e3c-9cb4-99ddf96f48f1', pg_temp.u('c', i), '96f859ef-a1ca-4b22-9372-c86ad58b694b', pg_temp.sp())
FROM generate_series(1, 14) i WHERE random() < 0.9;
SELECT pg_temp.rel('8f151ba4-de20-4e3c-9cb4-99ddf96f48f1', pg_temp.u('d', i), 'fd51f935-2063-4617-be39-7b672b23364c', pg_temp.sp())
FROM generate_series(1, 10) i, generate_series(1, 1 + (random() < 0.3)::int) j WHERE random() < 0.9;
SELECT pg_temp.rel('8f151ba4-de20-4e3c-9cb4-99ddf96f48f1', pg_temp.u('d', i), '96f859ef-a1ca-4b22-9372-c86ad58b694b', pg_temp.sp())
FROM generate_series(1, 10) i WHERE random() < 0.15;
SELECT pg_temp.rel('8f151ba4-de20-4e3c-9cb4-99ddf96f48f1', pg_temp.u('topic', i), '5ef5a586-0f27-4d8e-8f6c-59ae5b3e89e2', pg_temp.sp())
FROM generate_series(1, 8) i WHERE random() < 0.9;
SELECT pg_temp.rel('8f151ba4-de20-4e3c-9cb4-99ddf96f48f1', pg_temp.u('a', i), '79e3757b-2630-492c-9d3b-d73a78d45893', pg_temp.u('sp', 1 + (i % 4)))
FROM generate_series(1, 8) i;

-- Date values on agendas, in their space: varied spellings, an occasional invalid or second value.
INSERT INTO public."values" (id, property_id, entity_id, space_id, text, datetime_utc, datetime, date)
SELECT md5(:'seed' || 'v' || nextval('pg_temp.ids')), '52665f3e-fb7d-48d5-8b6b-abb21b0d36db'::uuid, pg_temp.u('a', i), pg_temp.u('sp', 1 + (i % 4)),
       NULL,
       CASE WHEN form = 3 THEN (DATE '2026-11-06' + off)::timestamptz + interval '3 hours' END,
       CASE form WHEN 0 THEN to_char(DATE '2026-11-06' + off, 'YYYY-MM-DD') || 'T00:00:00-05:00'
                 WHEN 4 THEN '2026-13-99T00:00:00Z' END,
       CASE form WHEN 1 THEN to_char(DATE '2026-11-06' + off, 'YYYY-MM-DD') WHEN 2 THEN to_char(DATE '2026-11-06' + off, 'YYYY-MM-DD') || 'T00:00:00Z' END
FROM (SELECT i, floor(random() * 5)::int AS form, (floor(random() * 17)::int - 12) AS off FROM generate_series(1, 8) i) x
UNION ALL
SELECT md5(:'seed' || 'v2' || nextval('pg_temp.ids')), '52665f3e-fb7d-48d5-8b6b-abb21b0d36db'::uuid, pg_temp.u('a', i), pg_temp.u('sp', 1 + (i % 4)),
       NULL, NULL, to_char(DATE '2026-11-06' + (floor(random() * 17)::int - 12), 'YYYY-MM-DD'), NULL
FROM generate_series(1, 8) i WHERE random() < 0.2;

-- Agenda relations: Blocks, Claims, Debates, Topics; mostly in the agenda's own space, a duplicate target now and then.
SELECT pg_temp.rel(t.p, pg_temp.u('a', a), pg_temp.u(t.k, 1 + floor(random() * t.n)::int),
                   CASE WHEN random() < 0.9 THEN pg_temp.u('sp', 1 + (a % 4)) ELSE pg_temp.sp() END)
FROM generate_series(1, 8) a,
     (VALUES ('beaba5cb-a677-41a8-b353-77030613fc70'::uuid, 'b', 10), ('e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid, 'c', 14),
             ('e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid, 'd', 10), ('b96bf701-a399-430d-a072-f5e910cdeda9'::uuid, 'd', 10),
             ('b96bf701-a399-430d-a072-f5e910cdeda9'::uuid, 'c', 14), ('806d52bc-27e9-4c91-93c0-57978b093351'::uuid, 'topic', 8)) t(p, k, n),
     generate_series(1, 3) rep
WHERE random() < 0.35;

-- Block contents, names and topics.
SELECT pg_temp.rel('a99f9ce1-2ffa-4dac-8c61-f6310d46064a', pg_temp.u('b', b), pg_temp.u(CASE WHEN random() < 0.7 THEN 'c' ELSE 'd' END, 1 + floor(random() * 10)::int),
                   CASE WHEN random() < 0.9 THEN pg_temp.dao() ELSE pg_temp.sp() END)
FROM generate_series(1, 10) b, generate_series(1, 5) rep WHERE random() < 0.5;
INSERT INTO public."values" (id, property_id, entity_id, space_id, text)
SELECT md5(:'seed' || 'n' || nextval('pg_temp.ids')), 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid, pg_temp.u('b', b), pg_temp.u('sp', s), 'Block ' || b || '-' || s
FROM generate_series(1, 10) b, generate_series(1, 3) s WHERE random() < 0.5;
SELECT pg_temp.rel('806d52bc-27e9-4c91-93c0-57978b093351', pg_temp.u('b', b), pg_temp.u('topic', 1 + floor(random() * 8)::int), pg_temp.dao())
FROM generate_series(1, 10) b, generate_series(1, 2) rep WHERE random() < 0.3;

-- Debates on claims (Claims relations from the debate), most with a Debate Types row in the same space.
WITH dl AS (SELECT pg_temp.u('d', d) AS d, pg_temp.u('c', c) AS c, pg_temp.sp() AS s
            FROM generate_series(1, 10) d, generate_series(1, 14) c WHERE random() < 0.12)
SELECT pg_temp.rel('e614cce1-c4ce-4586-8304-fd1237119eb2', d, c, s) FROM dl;
SELECT pg_temp.rel('8f151ba4-de20-4e3c-9cb4-99ddf96f48f1', r.from_entity_id, 'fd51f935-2063-4617-be39-7b672b23364c', r.space_id)
FROM public.relations r WHERE r.type_id = 'e614cce1-c4ce-4586-8304-fd1237119eb2' AND random() < 0.7
  AND r.from_entity_id IN (SELECT id FROM public.entities WHERE id::text IS NOT NULL) AND random() < 1;

-- Topics on claims and debates (any space), for entity_topic_listings.
SELECT pg_temp.rel('806d52bc-27e9-4c91-93c0-57978b093351', pg_temp.u(CASE WHEN random() < 0.6 THEN 'c' ELSE 'd' END, 1 + floor(random() * 10)::int),
                   pg_temp.u('topic', 1 + floor(random() * 8)::int), pg_temp.sp())
FROM generate_series(1, 30);

-- Names for the topic entities (spaces' own topics share names with some others), for the suggestions read.
INSERT INTO public."values" (id, property_id, entity_id, space_id, text)
SELECT md5(:'seed' || 'tn' || nextval('pg_temp.ids')), 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid, pg_temp.u('topic', t), pg_temp.u('sp', 1 + floor(random() * 3)::int),
       (ARRAY['AI', '  Mental   Health ', 'mental health', 'Space', 'AI', 'Cooking', E'Deep\tLearning', ' ', 'Music'])[1 + (t % 9)]
FROM generate_series(1, 8) t WHERE random() < 0.9;

-- The listings table as the hourly refresh leaves it: the function's rows.
INSERT INTO trending.agenda_listings SELECT agenda_id, space_id, entity_id, listed_on, source FROM trending.current_agenda_listings();
