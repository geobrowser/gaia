-- GEO-3503: one API query for the current Debate agendas in a set of spaces.
--
-- WHY. Five web surfaces need today's agendas (the debate-again picker, the side panel and hub
-- section, the lobby tab, the daily callout and the Trending tag). Each would otherwise walk agenda →
-- blocks → collection items itself and repeat the rules ranking already applies (0113), and they
-- could disagree. This is the one read they share.
--
-- WHAT IT RETURNS. `currentDebateAgendas(spaceIds, earlierDays)`: one row per item, already ordered.
-- spaceIds NULL means every space with an agenda this week.
--
--   The current agenda of each space: its freshest agenda whose Date has arrived in New York, dated
--   in the last week (today and the 6 days before, the web's "this week"); a space with none has no
--   current agenda. When a space has two agendas for the same Date, the later-created entity is
--   current (created_at is in seconds, so two created in the same block fall back to the larger id).
--   An agenda counts as current as soon as it exists, even before its blocks hold a claim.
--     source 'block_item'     the agenda's contents: each data block (a section, usually one topic)
--                             in order, and each block's claims in order, once each. The only rows a
--                             surface should show as the agenda.
--     source 'agenda_claims'  claims the agenda names in its own Claims relation, and
--     source 'agenda_topics'  topics it names in its own Topics relation: ranking levers (0113),
--                             returned so a surface can use them, never as agenda items.
--   Earlier listings (only with earlierDays > 0, at most 31): block items of the same spaces' other
--   agendas dated in the last earlierDays days, for "Earlier this week". A claim on its own space's
--   current agenda is left out (as the web hook does), and each claim appears once, at its newest
--   listing.
--
--   listedOn is the agenda's Date as a New York day and listedAt its midnight New York, the instant
--   a debate starts counting as "since the listing". Every claim row carries lastListedOn: the
--   claim's newest arrived claim listing in any space, which the Trending tag and "Earlier this week"
--   read. It comes from the listings table the hourly sweep keeps (as ranking does) and the row in
--   hand, so a new listing shows at once but a removed one can linger for up to an hour.
--
-- RULES, shared with ranking by building on 0113's helpers: agendas only in DAO spaces, a Date read
-- as the New York calendar day written, nothing until its Date arrives, block items and Claims
-- targets only when typed Claim, and every relation written in the agenda's own space. Agendas are
-- read live from the graph, so a new agenda shows at once.
--
-- ORDER. Spaces in the order asked (pass member spaces first; with spaceIds NULL, freshest agenda
-- first), each space's current agenda before its earlier listings, newest first; within an agenda,
-- block items, then its Claims, then its Topics; sections and items in the order Geo shows them.
--
-- Not PARALLEL SAFE, unlike earlier public reads: trending.value_date (0113) parses defensively in an
-- EXCEPTION block, which a parallel worker cannot run.

-- A relation position's sort key, in the order Geo shows relations: geo-sdk's Position.compare is
-- localeCompare, which for fractional-index keys (0-9A-Za-z) compares case-insensitively first and
-- puts lowercase first on a tie. This key does the same byte by byte (COLLATE "C"), so it does not
-- depend on the database's collation or on ICU. Null positions sort last.
CREATE OR REPLACE FUNCTION trending.position_order(pos text)
RETURNS text
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
  SELECT (lower(pos) || ' '
          || translate(pos, 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ',
                                 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz')) COLLATE "C"
$$;
--> statement-breakpoint

-- What the agendas in `space_ids` dated `since` or later name now (NULL: every space, every date),
-- in their own space, with where it sits: one row per block item (section = the block, in block
-- order, each claim once per block), per Claims target and per Topics target. Block items and Claims
-- targets are claims only. trending.current_agenda_listings (0113) is every agenda's rows without
-- their positions.
CREATE OR REPLACE FUNCTION trending.agenda_contents(space_ids uuid[] DEFAULT NULL, since date DEFAULT NULL)
RETURNS TABLE (agenda_id uuid, space_id uuid, listed_on date, source text, section_id uuid,
               section_position integer, entity_id uuid, item_position integer)
LANGUAGE sql STABLE AS $$
  WITH agendas AS (
    SELECT a.* FROM trending.current_agendas() a
    WHERE (space_ids IS NULL OR a.space_id = ANY(space_ids))
      AND (since IS NULL OR a.listed_on >= since)
  ),
  -- The agenda's own Blocks, Claims and Topics relations, each target once, at its first position:
  -- a block linked twice is one section, a claim named twice one lever.
  links AS (
    SELECT DISTINCT ON (a.agenda_id, a.space_id, r.type_id, r.to_entity_id)
           a.agenda_id, a.space_id, a.listed_on, r.type_id, r.to_entity_id,
           trending.position_order(r.position) AS link_order, r.id AS link_rel
    FROM agendas a
    JOIN public.relations r
      ON r.from_entity_id = a.agenda_id AND r.space_id = a.space_id
     AND r.type_id IN ('beaba5cb-a677-41a8-b353-77030613fc70'::uuid,                -- Blocks
                       'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid,                -- Claims
                       '806d52bc-27e9-4c91-93c0-57978b093351'::uuid)                -- Topics
    ORDER BY a.agenda_id, a.space_id, r.type_id, r.to_entity_id,
             trending.position_order(r.position) NULLS LAST, r.id
  ),
  blocks AS (
    SELECT l.agenda_id, l.space_id, l.listed_on, l.to_entity_id AS section_id,
           row_number() OVER (PARTITION BY l.agenda_id, l.space_id
                              ORDER BY l.link_order NULLS LAST, l.link_rel)::integer AS section_position
    FROM links l
    WHERE l.type_id = 'beaba5cb-a677-41a8-b353-77030613fc70'::uuid                   -- Blocks
  ),
  -- A claim dropped into one block twice is one item, at its first position.
  block_items AS (
    SELECT DISTINCT ON (b.agenda_id, b.space_id, b.section_id, item.to_entity_id)
           b.*, item.to_entity_id AS entity_id, trending.position_order(item.position) AS item_order, item.id AS item_rel
    FROM blocks b
    JOIN public.relations item
      ON item.from_entity_id = b.section_id AND item.space_id = b.space_id
     AND item.type_id = 'a99f9ce1-2ffa-4dac-8c61-f6310d46064a'::uuid                -- Collection item
    ORDER BY b.agenda_id, b.space_id, b.section_id, item.to_entity_id,
             trending.position_order(item.position) NULLS LAST, item.id
  ),
  items AS (
    SELECT bi.agenda_id, bi.space_id, bi.listed_on, 'block_item'::text AS source, bi.section_id,
           bi.section_position, bi.entity_id,
           row_number() OVER (PARTITION BY bi.agenda_id, bi.space_id, bi.section_id
                              ORDER BY bi.item_order NULLS LAST, bi.item_rel)::integer AS item_position
    FROM block_items bi
    UNION ALL
    SELECT l.agenda_id, l.space_id, l.listed_on,
           CASE l.type_id WHEN 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid THEN 'agenda_claims'
                          ELSE 'agenda_topics' END,
           NULL::uuid, NULL::integer, l.to_entity_id,
           row_number() OVER (PARTITION BY l.agenda_id, l.space_id, l.type_id
                              ORDER BY l.link_order NULLS LAST, l.link_rel)::integer
    FROM links l
    WHERE l.type_id <> 'beaba5cb-a677-41a8-b353-77030613fc70'::uuid                  -- not Blocks
  )
  SELECT i.agenda_id, i.space_id, i.listed_on, i.source, i.section_id, i.section_position,
         i.entity_id, i.item_position
  FROM items i
  LEFT JOIN trending.claims_and_debates(ARRAY(SELECT DISTINCT entity_id FROM items
                                              WHERE source <> 'agenda_topics')) k
    ON k.entity_id = i.entity_id
  WHERE i.source = 'agenda_topics' OR k.is_claim
$$;
--> statement-breakpoint

-- 0113's listings, now read from agenda_contents so the API and ranking cannot disagree on what an
-- agenda names. Same rows: positions dropped, so a claim in two blocks is one listing.
CREATE OR REPLACE FUNCTION trending.current_agenda_listings()
RETURNS TABLE (agenda_id uuid, space_id uuid, entity_id uuid, listed_on date, source text)
LANGUAGE sql STABLE AS $$
  SELECT DISTINCT c.agenda_id, c.space_id, c.entity_id, c.listed_on, c.source
  FROM trending.agenda_contents() c
$$;
--> statement-breakpoint

CREATE OR REPLACE FUNCTION public.current_debate_agendas(space_ids uuid[] DEFAULT NULL, earlier_days integer DEFAULT 0)
RETURNS TABLE (space_id uuid, agenda_id uuid, listed_on date, listed_at timestamptz, is_current boolean,
               source text, section_id uuid, section_name text, section_topic_id uuid,
               section_position integer, entity_id uuid, item_position integer, last_listed_on date)
LANGUAGE sql STABLE AS $$
  WITH params AS (
    SELECT trending.today() AS today,
           LEAST(GREATEST(COALESCE(earlier_days, 0), 0), 31) AS earlier,
           6 AS current_days                                                 -- today and 6 before
  ),
  -- Every arrived agenda in the asked spaces old enough to matter, ranked per space: newest Date
  -- first, the later-created agenda first on the same Date.
  ranked AS (
    SELECT a.agenda_id, a.space_id, a.listed_on,
           row_number() OVER (PARTITION BY a.space_id
                              ORDER BY a.listed_on DESC, e.created_at DESC NULLS LAST, a.agenda_id DESC) AS recency
    FROM trending.current_agendas() a
    JOIN public.entities e ON e.id = a.agenda_id
    CROSS JOIN params p
    WHERE (space_ids IS NULL OR a.space_id = ANY(space_ids))
      AND a.listed_on <= p.today
      AND a.listed_on >= p.today - GREATEST(p.current_days, p.earlier)
  ),
  -- With no spaces asked, only spaces with an agenda this week, whatever the earlier window.
  agendas AS (
    SELECT r.* FROM ranked r
    WHERE space_ids IS NOT NULL
       OR EXISTS (SELECT 1 FROM ranked n CROSS JOIN params p
                  WHERE n.space_id = r.space_id AND n.recency = 1 AND n.listed_on >= p.today - p.current_days)
  ),
  current_agendas AS (
    SELECT a.* FROM agendas a CROSS JOIN params p
    WHERE a.recency = 1 AND a.listed_on >= p.today - p.current_days
  ),
  -- The order spaces come back in: as asked, or freshest current agenda first.
  space_order AS (
    SELECT s.space_id,
           row_number() OVER (ORDER BY asked.n NULLS LAST, s.listed_on DESC NULLS LAST, s.space_id) AS n
    FROM (SELECT DISTINCT ON (a.space_id) a.space_id, ca.listed_on
          FROM agendas a LEFT JOIN current_agendas ca ON ca.space_id = a.space_id
          ORDER BY a.space_id) s
    LEFT JOIN (SELECT u.id, min(u.n) AS n FROM unnest(space_ids) WITH ORDINALITY AS u(id, n) GROUP BY u.id) asked
      ON asked.id = s.space_id
  ),
  contents AS (
    SELECT c.*
    FROM params p
    CROSS JOIN LATERAL trending.agenda_contents(
      ARRAY(SELECT DISTINCT a.space_id FROM agendas a), p.today - GREATEST(p.current_days, p.earlier)) c
  ),
  current_rows AS (
    SELECT c.*, true AS is_current, 1::bigint AS recency
    FROM contents c
    JOIN current_agendas ca ON ca.agenda_id = c.agenda_id AND ca.space_id = c.space_id
  ),
  -- Earlier block items: once per claim, at its newest listing; none that is on its own space's
  -- current agenda (as the web hook), so a space's rows do not depend on which others are asked.
  earlier_rows AS (
    SELECT DISTINCT ON (c.entity_id) c.*, false AS is_current, a.recency
    FROM contents c
    JOIN agendas a ON a.agenda_id = c.agenda_id AND a.space_id = c.space_id
    JOIN space_order so ON so.space_id = c.space_id
    CROSS JOIN params p
    WHERE p.earlier > 0
      AND c.source = 'block_item'
      AND c.listed_on >= p.today - p.earlier
      AND NOT EXISTS (SELECT 1 FROM current_agendas ca WHERE ca.agenda_id = c.agenda_id AND ca.space_id = c.space_id)
      AND NOT EXISTS (SELECT 1 FROM current_rows cr
                      WHERE cr.entity_id = c.entity_id AND cr.space_id = c.space_id AND cr.source = 'block_item')
    ORDER BY c.entity_id, c.listed_on DESC, so.n, a.recency, c.section_position, c.item_position
  ),
  picked AS (
    SELECT * FROM current_rows
    UNION ALL
    SELECT * FROM earlier_rows
  ),
  listed AS (
    SELECT l.claim_id, l.last_listed_on
    FROM trending.claim_listings(ARRAY(SELECT DISTINCT p.entity_id FROM picked p
                                       WHERE p.source <> 'agenda_topics')) l
  )
  SELECT p.space_id, p.agenda_id, p.listed_on,
         p.listed_on::timestamp AT TIME ZONE trending.time_zone(),
         p.is_current, p.source, p.section_id,
         (SELECT v.text FROM public.values v
          WHERE v.entity_id = p.section_id AND v.space_id = p.space_id
            AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid         -- Name
          LIMIT 1),
         (SELECT tp.to_entity_id FROM public.relations tp
          WHERE tp.from_entity_id = p.section_id AND tp.space_id = p.space_id
            AND tp.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid           -- Topics
          ORDER BY trending.position_order(tp.position) NULLS LAST, tp.id
          LIMIT 1),
         p.section_position, p.entity_id, p.item_position,
         -- The listings table trails a new agenda by up to an hour; the rows in hand are listings too,
         -- so a claim reads the same on every row of the response.
         CASE WHEN p.source <> 'agenda_topics'
              THEN GREATEST(l.last_listed_on,
                            max(p.listed_on) FILTER (WHERE p.source <> 'agenda_topics') OVER (PARTITION BY p.entity_id)) END
  FROM picked p
  JOIN space_order so ON so.space_id = p.space_id
  LEFT JOIN listed l ON l.claim_id = p.entity_id
  ORDER BY so.n, p.is_current DESC, p.listed_on DESC, p.recency,
           CASE p.source WHEN 'block_item' THEN 0 WHEN 'agenda_claims' THEN 1 ELSE 2 END,
           p.section_position, p.item_position
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.current_debate_agendas(uuid[], integer) IS
  E'The current Debate agendas (GEO-3503), one row per item, in order. Pass spaces in the order to show them (member spaces first), or none for every space with an agenda this week. Per space: its freshest agenda dated in the last week whose Date has arrived (the newer on a tie), with source block_item rows as its contents (sections and claims in the order Geo shows them) and agenda_claims / agenda_topics rows for the claims and topics it names as ranking levers, which are not agenda items. With earlierDays (up to 31), also the block items of these spaces'' other agendas in that many past days (isCurrent false), once per claim at its newest listing and none that is on its own space''s current agenda. listedAt is the agenda Date''s midnight in New York. lastListedOn is the claim''s newest listing in any space, kept hourly. Like every list, it returns 100 rows unless first asks for more (up to 1000).';
--> statement-breakpoint

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'gaia_app') AND current_user <> 'gaia_app' THEN
    EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA trending TO gaia_app';
  END IF;
END
$$;
