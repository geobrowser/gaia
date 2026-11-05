-- GEO-3592: a Debate agenda can name single debates, through its own Debates relation, and Best lifts
-- just those debates.
--
-- WHY. A curator can lift a claim (its Claims relation or a block item) and a topic (its Topics
-- relation), but not one debate. A debate worth showing moved up only when its claim was listed, and
-- that lifts every debate on the claim, weak ones included (0113 gives them all the same boost).
--
-- WHAT IT NAMES. The agenda's own Debates relation (b96bf701) → debate, under 0113's rules for the
-- agenda's other relations: the agenda in a DAO space, the relation written in the agenda's own
-- space, and the target typed Debate (in any space, as a Claims target must be typed Claim). Stored in
-- trending.agenda_listings as source 'agenda_debates', with entity_id the debate.
--
-- HOW IT RANKS. A debate listing is the debate's own listing, scored exactly as 0113 scores a debate
-- on a listed claim: trending = boost × day_factor^days and the listing-day age credit, so an old
-- debate comes back up. That is the full listing boost. 0113 already gave every debate on a listed
-- claim the full boost too, with no debate discount (debate_factor only discounts claims). What is new
-- is that only the named debate moves. A debate both listed itself and on a listed claim takes the
-- newer of the two listings.
--
-- WHAT IT DOES NOT DO. It is not a claim listing: the debate's claim, and the other debates on it,
-- get nothing from it, and it never counts toward a claim's lastListedOn, listedOnAgendaId or 0113's
-- "debates on a listed claim". It is not an agenda item either. currentDebateAgendas returns it as a
-- ranking lever, as it does agenda_claims and agenda_topics rows, and every Today's claims surface
-- reads only block_item rows.
--
-- THE TRENDING TAG. The tag reads lastListedOn, which for a debate is now the newer of its own
-- listing and its claim's. A named debate is tagged for as long as a listing would keep it, and its
-- claim is not.
--
-- FRESHNESS, as 0113's: the hourly sweep re-scores debates whose agenda_debates rows changed, and the
-- daily full pass re-scores every listed debate. A debate that loses its Debate type keeps its terms
-- until the next full pass, which clears them as a stored term.
--
-- WAREHOUSE. GEO-3556's export (0117, if merged) sends every source. Analytics' claim view keeps
-- only block_item and agenda_claims, so the new rows are exported and stay out of claim reports.

ALTER TABLE "trending"."agenda_listings" DROP CONSTRAINT IF EXISTS "agenda_listings_source_check";
--> statement-breakpoint
ALTER TABLE "trending"."agenda_listings" ADD CONSTRAINT "agenda_listings_source_check"
  CHECK ("source" IN ('block_item', 'agenda_claims', 'agenda_debates', 'agenda_topics'));
--> statement-breakpoint

-- 0115's contents, plus one row per Debates target typed Debate (source 'agenda_debates', numbered
-- like the Claims targets). Everything else is unchanged.
CREATE OR REPLACE FUNCTION trending.agenda_contents(space_ids uuid[] DEFAULT NULL, since date DEFAULT NULL)
RETURNS TABLE (agenda_id uuid, space_id uuid, listed_on date, source text, section_id uuid,
               section_position integer, entity_id uuid, item_position integer)
LANGUAGE sql STABLE AS $$
  WITH agendas AS (
    SELECT a.* FROM trending.current_agendas() a
    WHERE (space_ids IS NULL OR a.space_id = ANY(space_ids))
      AND (since IS NULL OR a.listed_on >= since)
  ),
  -- The agenda's own Blocks, Claims, Debates and Topics relations, each target once, at its first
  -- position: a block linked twice is one section, a claim named twice one lever.
  links AS (
    SELECT DISTINCT ON (a.agenda_id, a.space_id, r.type_id, r.to_entity_id)
           a.agenda_id, a.space_id, a.listed_on, r.type_id, r.to_entity_id,
           trending.position_order(r.position) AS link_order, r.id AS link_rel
    FROM agendas a
    JOIN public.relations r
      ON r.from_entity_id = a.agenda_id AND r.space_id = a.space_id
     AND r.type_id IN ('beaba5cb-a677-41a8-b353-77030613fc70'::uuid,                -- Blocks
                       'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid,                -- Claims
                       'b96bf701-a399-430d-a072-f5e910cdeda9'::uuid,                -- Debates
                       '806d52bc-27e9-4c91-93c0-57978b093351'::uuid)                -- Topics
    ORDER BY a.agenda_id, a.space_id, r.type_id, r.to_entity_id,
             trending.position_order(r.position) NULLS LAST, r.id
  ),
  -- Each block's items, a claim dropped into one block twice once, at its first position.
  block_items AS (
    SELECT DISTINCT ON (l.agenda_id, l.space_id, l.to_entity_id, item.to_entity_id)
           l.agenda_id, l.space_id, l.listed_on, l.to_entity_id AS section_id,
           l.link_order AS section_order, l.link_rel AS section_rel, item.to_entity_id AS entity_id,
           trending.position_order(item.position) AS item_order, item.id AS item_rel
    FROM links l
    JOIN public.relations item
      ON item.from_entity_id = l.to_entity_id AND item.space_id = l.space_id
     AND item.type_id = 'a99f9ce1-2ffa-4dac-8c61-f6310d46064a'::uuid                -- Collection item
    WHERE l.type_id = 'beaba5cb-a677-41a8-b353-77030613fc70'::uuid                   -- Blocks
    ORDER BY l.agenda_id, l.space_id, l.to_entity_id, item.to_entity_id,
             trending.position_order(item.position) NULLS LAST, item.id
  ),
  -- Block items and Claims targets typed Claim, and Debates targets typed Debate: everything else
  -- drops out before numbering.
  kinds AS (
    SELECT * FROM trending.claims_and_debates(ARRAY(
      SELECT bi.entity_id FROM block_items bi
      UNION
      SELECT l.to_entity_id FROM links l
      WHERE l.type_id IN ('e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid,              -- Claims
                          'b96bf701-a399-430d-a072-f5e910cdeda9'::uuid)))            -- Debates
  ),
  claims AS (SELECT k.entity_id FROM kinds k WHERE k.is_claim),
  debates AS (SELECT k.entity_id FROM kinds k WHERE k.is_debate)
  SELECT bi.agenda_id, bi.space_id, bi.listed_on, 'block_item'::text, bi.section_id,
         dense_rank() OVER (PARTITION BY bi.agenda_id, bi.space_id
                            ORDER BY bi.section_order NULLS LAST, bi.section_rel)::integer,
         bi.entity_id,
         row_number() OVER (PARTITION BY bi.agenda_id, bi.space_id, bi.section_id
                            ORDER BY bi.item_order NULLS LAST, bi.item_rel)::integer
  FROM block_items bi
  JOIN claims c ON c.entity_id = bi.entity_id
  UNION ALL
  SELECT l.agenda_id, l.space_id, l.listed_on,
         CASE l.type_id WHEN 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid THEN 'agenda_claims'
                        WHEN 'b96bf701-a399-430d-a072-f5e910cdeda9'::uuid THEN 'agenda_debates'
                        ELSE 'agenda_topics' END,
         NULL::uuid, NULL::integer, l.to_entity_id,
         row_number() OVER (PARTITION BY l.agenda_id, l.space_id, l.type_id
                            ORDER BY l.link_order NULLS LAST, l.link_rel)::integer
  FROM links l
  WHERE l.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid                     -- Topics
     OR (l.type_id = 'e614cce1-c4ce-4586-8304-fd1237119eb2'::uuid                    -- Claims
         AND EXISTS (SELECT 1 FROM claims c WHERE c.entity_id = l.to_entity_id))
     OR (l.type_id = 'b96bf701-a399-430d-a072-f5e910cdeda9'::uuid                    -- Debates
         AND EXISTS (SELECT 1 FROM debates d WHERE d.entity_id = l.to_entity_id))
$$;
--> statement-breakpoint

-- 0113's, with the claim sources named: a debate listing is not a claim listing, any more than a topic
-- listing is.
CREATE OR REPLACE FUNCTION trending.claim_listings(claim_ids uuid[])
RETURNS TABLE (claim_id uuid, last_listed_on date, agenda_id uuid, space_id uuid)
LANGUAGE sql STABLE AS $$
  SELECT DISTINCT ON (l.entity_id) l.entity_id, l.listed_on, l.agenda_id, l.space_id
  FROM trending.agenda_listings l
  WHERE l.entity_id = ANY(claim_ids) AND l.source IN ('block_item', 'agenda_claims')
    AND l.listed_on <= trending.today()
  ORDER BY l.entity_id, l.listed_on DESC, l.agenda_id
$$;
--> statement-breakpoint

-- Per debate, its newest arrived listing of its own (an agenda's Debates relation).
CREATE OR REPLACE FUNCTION trending.debate_listings(debate_ids uuid[])
RETURNS TABLE (debate_id uuid, last_listed_on date, agenda_id uuid, space_id uuid)
LANGUAGE sql STABLE AS $$
  SELECT DISTINCT ON (l.entity_id) l.entity_id, l.listed_on, l.agenda_id, l.space_id
  FROM trending.agenda_listings l
  WHERE l.entity_id = ANY(debate_ids) AND l.source = 'agenda_debates'
    AND l.listed_on <= trending.today()
  ORDER BY l.entity_id, l.listed_on DESC, l.agenda_id
$$;
--> statement-breakpoint

-- 0113's, plus a debate's own listing: per claim or debate, the newest arrived listing that counts for
-- it: its own if it is a claim, its own or its Claims' if it is a debate. A claim never takes a debate
-- listing, so naming a debate lifts neither its claim nor the claim's other debates.
CREATE OR REPLACE FUNCTION trending.entity_listings(entity_ids uuid[])
RETURNS TABLE (entity_id uuid, last_listed_on date, agenda_id uuid)
LANGUAGE sql STABLE AS $$
  WITH k AS (SELECT * FROM trending.claims_and_debates(entity_ids)),
  claim_of AS (
    SELECT k.entity_id, k.entity_id AS claim_id FROM k WHERE k.is_claim
    UNION
    SELECT dl.debate_id, dl.claim_id
    FROM trending.debate_links('{}', ARRAY(SELECT k.entity_id FROM k WHERE k.is_debate)) dl
  ),
  listings AS (
    SELECT co.entity_id, l.last_listed_on, l.agenda_id
    FROM claim_of co
    JOIN trending.claim_listings(ARRAY(SELECT claim_id FROM claim_of)) l ON l.claim_id = co.claim_id
    UNION ALL
    SELECT dl.debate_id, dl.last_listed_on, dl.agenda_id
    FROM trending.debate_listings(ARRAY(SELECT k.entity_id FROM k WHERE k.is_debate)) dl
  )
  SELECT DISTINCT ON (x.entity_id) x.entity_id, x.last_listed_on, x.agenda_id
  FROM listings x
  ORDER BY x.entity_id, x.last_listed_on DESC, x.agenda_id
$$;
--> statement-breakpoint

-- 0113's sweep, plus the debates whose own listings changed (and, on a full pass, every listed
-- debate). Claim sources are named, so a debate listing never fans out to its claim's debates.
CREATE OR REPLACE FUNCTION trending.refresh_agenda_listings(full_pass boolean DEFAULT NULL)
RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
  claim_ids uuid[];
  debate_ids uuid[];
  topic_ids uuid[];
  ids uuid[];
  horizon integer;
  boost numeric;
  today date;
BEGIN
  -- One writer at a time; the CronJob forbids overlap too.
  PERFORM pg_advisory_xact_lock(hashtext('trending.agenda_listings'));
  SELECT c.trending_horizon_days, c.trending_boost, trending.today() INTO horizon, boost, today
  FROM public.entity_ranking_config c WHERE c.id;
  full_pass := COALESCE(full_pass, (SELECT st.last_full_pass_on IS DISTINCT FROM today
                                    FROM trending.refresh_state st WHERE st.id));
  IF full_pass THEN
    UPDATE trending.refresh_state SET last_full_pass_on = today WHERE id;
  END IF;

  CREATE TEMP TABLE IF NOT EXISTS incoming_agenda_listings (
    agenda_id uuid, space_id uuid, entity_id uuid, listed_on date, source text
  ) ON COMMIT DROP;
  TRUNCATE incoming_agenda_listings;
  INSERT INTO incoming_agenda_listings SELECT * FROM trending.current_agenda_listings();

  CREATE TEMP TABLE IF NOT EXISTS changed_agenda_listings (
    agenda_id uuid, space_id uuid, entity_id uuid, listed_on date, source text, incoming boolean
  ) ON COMMIT DROP;
  TRUNCATE changed_agenda_listings;
  INSERT INTO changed_agenda_listings
  SELECT x.*, false FROM (
    SELECT l.agenda_id, l.space_id, l.entity_id, l.listed_on, l.source FROM trending.agenda_listings l
    EXCEPT SELECT * FROM incoming_agenda_listings) x
  UNION ALL
  SELECT x.*, true FROM (
    SELECT * FROM incoming_agenda_listings
    EXCEPT SELECT l.agenda_id, l.space_id, l.entity_id, l.listed_on, l.source FROM trending.agenda_listings l) x;

  -- Claims, debates and topics whose rows changed (added, removed or re-dated). A full pass adds every
  -- listed claim and debate, for the age credit, and topics inside the horizon: past it a topic lifts
  -- nothing.
  SELECT coalesce(array_agg(DISTINCT x.entity_id) FILTER (WHERE x.source IN ('block_item', 'agenda_claims')
                                                            AND (x.changed OR full_pass)), '{}'),
         coalesce(array_agg(DISTINCT x.entity_id) FILTER (WHERE x.source = 'agenda_debates'
                                                            AND (x.changed OR full_pass)), '{}'),
         coalesce(array_agg(DISTINCT x.entity_id) FILTER (WHERE x.source = 'agenda_topics'
                                                            AND (x.changed OR (full_pass AND x.listed_on BETWEEN today - horizon AND today))), '{}')
    INTO claim_ids, debate_ids, topic_ids
  FROM (
    SELECT c.entity_id, c.source, c.listed_on, true AS changed FROM changed_agenda_listings c
    UNION ALL
    SELECT i.entity_id, i.source, i.listed_on, false FROM incoming_agenda_listings i
  ) x;

  -- Claims listed inside the horizon with a debate that does not carry the boost yet (see 0113's
  -- FRESHNESS). Only while the boost is on: at 0 every such debate would match every hour.
  IF boost > 0 THEN
    claim_ids := claim_ids || ARRAY(
      SELECT DISTINCT dl.claim_id
      FROM trending.debate_links(ARRAY(
        SELECT i.entity_id FROM incoming_agenda_listings i
        WHERE i.source IN ('block_item', 'agenda_claims') AND i.listed_on BETWEEN today - horizon AND today), '{}') dl
      LEFT JOIN public.entity_ranking_scores s ON s.entity_id = dl.debate_id
      WHERE COALESCE(s.trending_score, 0) = 0);
  END IF;

  DELETE FROM trending.agenda_listings l
  USING changed_agenda_listings c
  WHERE NOT c.incoming
    AND (l.agenda_id, l.space_id, l.entity_id, l.listed_on, l.source)
      = (c.agenda_id, c.space_id, c.entity_id, c.listed_on, c.source);
  INSERT INTO trending.agenda_listings (agenda_id, space_id, entity_id, listed_on, source)
  SELECT c.agenda_id, c.space_id, c.entity_id, c.listed_on, c.source
  FROM changed_agenda_listings c WHERE c.incoming;

  -- Those claims and the debates on them; those debates; claims and debates tagged with those topics;
  -- and on a full pass anything still storing a term (a removed tag or Claims link, a config change),
  -- by the partial index.
  SELECT coalesce(array_agg(DISTINCT x.id), '{}') INTO ids
  FROM (
    SELECT unnest(claim_ids) AS id
    UNION ALL
    SELECT d.debate_id FROM trending.debate_links(claim_ids, '{}') d
    UNION ALL
    SELECT unnest(debate_ids)
    UNION ALL
    SELECT k.entity_id
    FROM trending.claims_and_debates(ARRAY(
      SELECT tg.from_entity_id FROM public.relations tg
      JOIN public.spaces sp ON sp.id = tg.space_id AND sp.type = 'DAO'
      WHERE tg.to_entity_id = ANY(topic_ids)
        AND tg.type_id = '806d52bc-27e9-4c91-93c0-57978b093351'::uuid)) k                -- Topics
    UNION ALL
    SELECT s.entity_id FROM public.entity_ranking_scores s
    WHERE full_pass AND (s.trending_score <> 0 OR s.age_credit <> 0)
  ) x;
  IF cardinality(ids) = 0 THEN
    RETURN 0;
  END IF;
  RETURN public.refresh_entity_ranking_scores(ids);
END;
$$;
--> statement-breakpoint

-- 0115's query. Changes: agenda_debates rows (from agenda_contents) carry the debate's lastListedOn,
-- its own listing or its claim's, and sort after agenda_claims. A claim row's lastListedOn reads
-- only claim rows.
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
                              ORDER BY a.listed_on DESC,
                                       CASE WHEN e.created_at ~ '^[0-9]+$' THEN e.created_at::bigint END DESC NULLS LAST,
                                       a.agenda_id DESC) AS recency
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
  -- Read once, each row carrying its agenda's recency (see 0115).
  contents AS MATERIALIZED (
    SELECT c.*, ag.recency
    FROM params p
    CROSS JOIN LATERAL trending.agenda_contents(
      ARRAY(SELECT DISTINCT a.space_id FROM agendas a), p.today - GREATEST(p.current_days, p.earlier)) c
    JOIN agendas ag ON ag.agenda_id = c.agenda_id AND ag.space_id = c.space_id
  ),
  current_rows AS (
    SELECT c.*, true AS is_current
    FROM contents c
    JOIN current_agendas ca ON ca.agenda_id = c.agenda_id AND ca.space_id = c.space_id
  ),
  -- Earlier block items: once per claim in each space, at its newest listing there; none that is on
  -- its own space's current agenda (see 0115).
  earlier_first AS (
    SELECT DISTINCT ON (c.space_id, c.entity_id) c.*
    FROM contents c
    CROSS JOIN params p
    WHERE p.earlier > 0
      AND c.source = 'block_item'
      AND c.listed_on >= p.today - p.earlier
      AND (c.space_id, c.entity_id) NOT IN (SELECT cr.space_id, cr.entity_id FROM current_rows cr
                                            WHERE cr.source = 'block_item')
    ORDER BY c.space_id, c.entity_id, c.listed_on DESC, c.recency, c.section_position, c.item_position
  ),
  -- Renumbered over what is returned, as the current agenda's rows are.
  earlier_rows AS (
    SELECT e.agenda_id, e.space_id, e.listed_on, e.source, e.section_id,
           dense_rank() OVER (PARTITION BY e.space_id, e.agenda_id ORDER BY e.section_position)::integer,
           e.entity_id,
           row_number() OVER (PARTITION BY e.space_id, e.agenda_id, e.section_id ORDER BY e.item_position)::integer,
           e.recency, false
    FROM earlier_first e
  ),
  picked AS (
    SELECT * FROM current_rows
    UNION ALL
    SELECT * FROM earlier_rows
  ),
  listed AS (
    SELECT l.claim_id, l.last_listed_on
    FROM trending.claim_listings(ARRAY(SELECT DISTINCT p.entity_id FROM picked p
                                       WHERE p.source IN ('block_item', 'agenda_claims'))) l
  ),
  debate_listed AS (
    SELECT l.entity_id, l.last_listed_on
    FROM trending.entity_listings(ARRAY(SELECT DISTINCT p.entity_id FROM picked p
                                        WHERE p.source = 'agenda_debates')) l
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
         -- so an entity reads the same on every row of the response.
         CASE WHEN p.source IN ('block_item', 'agenda_claims')
              THEN GREATEST(l.last_listed_on,
                            max(p.listed_on) FILTER (WHERE p.source IN ('block_item', 'agenda_claims'))
                              OVER (PARTITION BY p.entity_id))
              WHEN p.source = 'agenda_debates'
              THEN GREATEST(dl.last_listed_on,
                            max(p.listed_on) FILTER (WHERE p.source = 'agenda_debates')
                              OVER (PARTITION BY p.entity_id)) END
  FROM picked p
  JOIN space_order so ON so.space_id = p.space_id
  LEFT JOIN listed l ON l.claim_id = p.entity_id
  LEFT JOIN debate_listed dl ON dl.entity_id = p.entity_id
  ORDER BY so.n, p.is_current DESC, p.listed_on DESC, p.recency,
           CASE p.source WHEN 'block_item' THEN 0 WHEN 'agenda_claims' THEN 1 WHEN 'agenda_debates' THEN 2 ELSE 3 END,
           p.section_position, p.item_position
$$;
--> statement-breakpoint
COMMENT ON FUNCTION public.current_debate_agendas(uuid[], integer) IS
  E'The current Debate agendas (GEO-3503), one row per item, in order. Pass spaces in the order to show them (member spaces first), or none for every space with an agenda this week. Per space: its freshest agenda dated in the last week whose Date has arrived (the newer on a tie), with source block_item rows as its contents (sections and claims in the order Geo shows them) and agenda_claims / agenda_debates / agenda_topics rows for the claims, debates and topics it names as ranking levers, which are not agenda items. With earlierDays (up to 31), also the block items of these spaces'' other agendas in that many past days (isCurrent false), once per claim per space at its newest listing there and none that is on its own space''s current agenda; a space''s rows do not depend on which others are asked. Positions number only the rows returned. listedAt is the agenda Date''s midnight in New York. lastListedOn is the claim''s newest claim listing in any space (for an agenda_debates row, the debate''s own or its claim''s, whichever is newer), kept hourly. Like every list, it returns 100 rows unless first asks for more (up to 1000).';
--> statement-breakpoint

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'gaia_app') AND current_user <> 'gaia_app' THEN
    EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA trending TO gaia_app';
  END IF;
END
$$;
--> statement-breakpoint

-- No sweep here, as in 0113: the next hourly run picks up Debates links. Bump For you's feed revision
-- (0102) so its analytics can tell the rankings apart.
UPDATE "entity_ranking_config" SET "updated_at" = now() WHERE "id";
