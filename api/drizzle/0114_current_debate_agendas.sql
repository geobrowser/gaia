-- GEO-3503: one API query for the current Debate agendas in a set of spaces.
--
-- WHY. Five web surfaces need today's agendas (the debate-again picker, the side panel and hub
-- section, the lobby tab, the daily callout and the Trending tag). Each would otherwise walk agenda →
-- blocks → collection items itself and repeat the rules ranking already applies (0113), and they
-- could disagree. This is the one read they share.
--
-- WHAT IT RETURNS. `currentDebateAgendas(spaceIds, earlierDays)`: one row per item, already ordered.
--
--   The current agenda of each space: its freshest agenda whose Date has arrived in New York. When a
--   space has two agendas for the same Date, the newer one (the later-created entity) is current.
--     source 'block_item'     the agenda's contents: each data block (a section, usually one topic)
--                             in order, and each block's claims in order. The only rows a surface
--                             should show as the agenda.
--     source 'agenda_claims'  claims the agenda names in its own Claims relation, and
--     source 'agenda_topics'  topics it names in its own Topics relation: ranking levers (0113),
--                             returned so a surface can use them, never as agenda items.
--   Earlier listings (only with earlierDays > 0): block items of the same spaces' other agendas
--   dated in the last earlierDays days, for "Earlier this week". A claim on any requested space's
--   current agenda is left out, and each claim appears once, at its newest listing.
--
--   Every claim row carries lastListedOn: the claim's newest arrived claim listing in any space,
--   which is what the Trending tag and "Earlier this week" read.
--
-- RULES, shared with ranking by building on 0113's helpers: agendas only in DAO spaces, a Date read
-- as the New York calendar day written, nothing until its Date arrives, block items and Claims
-- targets only when typed Claim, and every relation written in the agenda's own space. The agenda is
-- read live from the graph (a new agenda shows at once); lastListedOn reads the listings table, which
-- the hourly sweep keeps, so it also takes the rows returned here into account.
--
-- ORDER. Spaces in the order asked (pass member spaces first), each space's current agenda before
-- its earlier listings, newest first; within an agenda, block items, then its Claims, then its
-- Topics; sections and items by their relation's position. Positions are fractional-index keys in
-- base 62 (0-9A-Za-z, in that order), so they are compared byte by byte (COLLATE "C").

-- Everything every agenda names now, in its own space, with where it sits: one row per block item
-- (section = the block, in block order), per Claims target and per Topics target. Block items and
-- Claims targets are claims only. trending.current_agenda_listings (0113) is these rows without
-- their positions.
CREATE OR REPLACE FUNCTION trending.agenda_contents()
RETURNS TABLE (agenda_id uuid, space_id uuid, listed_on date, source text, section_id uuid,
               section_position integer, entity_id uuid, item_position integer)
LANGUAGE sql STABLE AS $$
  WITH agendas AS (SELECT * FROM trending.current_agendas()),
  blocks AS (
    SELECT a.agenda_id, a.space_id, a.listed_on, blk.to_entity_id AS section_id,
           row_number() OVER (PARTITION BY a.agenda_id, a.space_id
                              ORDER BY blk.position COLLATE "C" NULLS LAST, blk.id)::integer AS section_position
    FROM agendas a
    JOIN public.relations blk
      ON blk.from_entity_id = a.agenda_id AND blk.space_id = a.space_id
     AND blk.type_id = 'beaba5cb-a677-41a8-b353-77030613fc70'::uuid                 -- Blocks
  ),
  items AS (
    SELECT b.agenda_id, b.space_id, b.listed_on, 'block_item'::text AS source, b.section_id,
           b.section_position, item.to_entity_id AS entity_id,
           row_number() OVER (PARTITION BY b.agenda_id, b.space_id, b.section_id
                              ORDER BY item.position COLLATE "C" NULLS LAST, item.id)::integer AS item_position
    FROM blocks b
    JOIN public.relations item
      ON item.from_entity_id = b.section_id AND item.space_id = b.space_id
     AND item.type_id = 'a99f9ce1-2ffa-4dac-8c61-f6310d46064a'::uuid                -- Collection item
    UNION ALL
    SELECT a.agenda_id, a.space_id, a.listed_on,
           CASE r.type_id WHEN 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid THEN 'agenda_claims'
                          ELSE 'agenda_topics' END,
           NULL::uuid, NULL::integer, r.to_entity_id,
           row_number() OVER (PARTITION BY a.agenda_id, a.space_id, r.type_id
                              ORDER BY r.position COLLATE "C" NULLS LAST, r.id)::integer
    FROM agendas a
    JOIN public.relations r
      ON r.from_entity_id = a.agenda_id AND r.space_id = a.space_id
     AND r.type_id IN ('e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid,                -- Claims
                       '806d52bc-27e9-4c91-93c0-57978b093351'::uuid)                -- Topics
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

CREATE OR REPLACE FUNCTION public.current_debate_agendas(space_ids uuid[], earlier_days integer DEFAULT 0)
RETURNS TABLE (space_id uuid, agenda_id uuid, listed_on date, is_current boolean, source text,
               section_id uuid, section_name text, section_topic_id uuid, section_position integer,
               entity_id uuid, item_position integer, last_listed_on date)
LANGUAGE sql STABLE AS $$
  WITH asked AS (
    SELECT s.id, min(s.n)::integer AS n
    FROM unnest(space_ids) WITH ORDINALITY AS s(id, n)
    GROUP BY s.id
  ),
  contents AS (
    SELECT c.*, asked.n AS space_n
    FROM trending.agenda_contents() c
    JOIN asked ON asked.id = c.space_id
    WHERE c.listed_on <= trending.today()
  ),
  -- Each (agenda, space), newest Date first, the later-created agenda first on the same Date.
  agendas AS (
    SELECT x.agenda_id, x.space_id, x.listed_on,
           row_number() OVER (PARTITION BY x.space_id
                              ORDER BY x.listed_on DESC, e.created_at DESC NULLS LAST, x.agenda_id DESC) AS recency
    FROM (SELECT DISTINCT agenda_id, space_id, listed_on FROM contents) x
    JOIN public.entities e ON e.id = x.agenda_id
  ),
  current_rows AS (
    SELECT c.*, true AS is_current
    FROM contents c
    JOIN agendas a ON a.agenda_id = c.agenda_id AND a.space_id = c.space_id AND a.recency = 1
  ),
  -- Earlier block items: once per claim, at its newest listing; none that is on a current agenda.
  earlier_rows AS (
    SELECT DISTINCT ON (c.entity_id) c.*, false AS is_current
    FROM contents c
    JOIN agendas a ON a.agenda_id = c.agenda_id AND a.space_id = c.space_id AND a.recency > 1
    WHERE earlier_days > 0
      AND c.source = 'block_item'
      AND c.listed_on >= trending.today() - earlier_days
      AND NOT EXISTS (SELECT 1 FROM current_rows cr
                      WHERE cr.entity_id = c.entity_id AND cr.source = 'block_item')
    ORDER BY c.entity_id, c.listed_on DESC, c.space_n, a.recency, c.section_position, c.item_position
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
  SELECT p.space_id, p.agenda_id, p.listed_on, p.is_current, p.source,
         p.section_id,
         (SELECT v.text FROM public.values v
          WHERE v.entity_id = p.section_id AND v.space_id = p.space_id
            AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid         -- Name
          LIMIT 1),
         (SELECT tp.to_entity_id FROM public.relations tp
          WHERE tp.from_entity_id = p.section_id AND tp.space_id = p.space_id
            AND tp.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid           -- Topics
          ORDER BY tp.position COLLATE "C" NULLS LAST, tp.id
          LIMIT 1),
         p.section_position, p.entity_id, p.item_position,
         -- The listings table trails a new agenda by up to an hour; the row in hand is a listing too.
         CASE WHEN p.source <> 'agenda_topics' THEN GREATEST(l.last_listed_on, p.listed_on) END
  FROM picked p
  JOIN agendas a ON a.agenda_id = p.agenda_id AND a.space_id = p.space_id
  LEFT JOIN listed l ON l.claim_id = p.entity_id
  ORDER BY p.space_n, p.is_current DESC, p.listed_on DESC, a.recency,
           CASE p.source WHEN 'block_item' THEN 0 WHEN 'agenda_claims' THEN 1 ELSE 2 END,
           p.section_position, p.item_position
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.current_debate_agendas(uuid[], integer) IS
  E'The current Debate agendas in these spaces (GEO-3503), one row per item, in order. Pass spaces in the order to show them (member spaces first). Per space: its freshest agenda whose Date has arrived (the newer on a tie), with source block_item rows as its contents (sections and claims in order) and agenda_claims / agenda_topics rows for the claims and topics it names as ranking levers, which are not agenda items. With earlierDays > 0, also the block items of these spaces'' other agendas in that many past days (isCurrent false), once per claim at its newest listing and none that is on a current agenda. lastListedOn is the claim''s newest listing in any space. Like every list, it returns 100 rows unless first asks for more (up to 1000).';
--> statement-breakpoint

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'gaia_app') AND current_user <> 'gaia_app' THEN
    EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA trending TO gaia_app';
  END IF;
END
$$;
