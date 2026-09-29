-- GEO-3079: ranked topic suggestions for the onboarding "Customize your feed" step.
--
-- A new user picks some spaces, then the next step offers topics to follow. The agreed ranking
-- is how many claims and debates in those spaces link to each topic through the Topics
-- property: broad topics first, niche ones further down. Counting that on the fly reads 4k-10k
-- links per space, too slow for a dialog that opens instantly, and the ranking barely changes
-- day to day. So it is precomputed into `space_topic_suggestions` by
-- `refresh_space_topic_suggestions()` (hourly, ranking-indexer's `topic_suggestions_refresh`
-- CronJob), and the dialog calls `topic_suggestions_for_spaces(space_ids)`, which is one
-- indexed read.
--
-- Measured on the live DB 2026-09-29: the full refresh is one pass over ~668k Topics relations
-- joined to ~366k claim/debate Types relations, 1.6s, producing ~5.2k (space, topic) rows.
--
-- CLEANUP happens at refresh time, so the read has nothing to filter:
--   * Only real topics: the target carries Types -> Topic and has a non-blank name.
--   * Not the space's own topic. Matched by id (`spaces.topic_id`) AND by name, because the AI
--     space's own topic is 8cb0a2b4 while its content also links a second entity named "AI"
--     (376 links, its #2 without this). A topic sharing the space's name is the space itself.
--   * One topic per name, ignoring case and whitespace, keeping the most-linked: Health has
--     two separate "Mental health" topics.
-- ACROSS SPACES, at read time: a topic appears once, at its best rank in any chosen space, and
-- results interleave by per-space rank so every chosen space is represented near the top
-- instead of the space with the most links filling the first page.

CREATE TABLE IF NOT EXISTS "space_topic_suggestions" (
	"space_id" uuid NOT NULL,
	"topic_id" uuid NOT NULL,
	-- The name lowercased, trimmed and with runs of whitespace collapsed: the key both
	-- de-duplications use.
	"name_key" text NOT NULL,
	-- Distinct claims and debates in the space linking to the topic.
	"links" integer NOT NULL,
	-- 1 = most linked in this space.
	"rank" integer NOT NULL,
	"refreshed_at" timestamptz NOT NULL DEFAULT now(),
	CONSTRAINT "space_topic_suggestions_pk" PRIMARY KEY ("space_id", "topic_id")
);
--> statement-breakpoint

CREATE INDEX IF NOT EXISTS "space_topic_suggestions_space_rank_idx"
	ON "space_topic_suggestions" ("space_id", "rank");
--> statement-breakpoint

-- Internal: read through topic_suggestions_for_spaces, never as a GraphQL table.
COMMENT ON TABLE "space_topic_suggestions" IS E'@omit';
--> statement-breakpoint

-- Rebuild the whole table. DELETE then INSERT inside one function call, so it is one
-- transaction: readers keep seeing the previous ranking until the new one commits, never a
-- half-built or empty table. Returns the number of rows written.
CREATE OR REPLACE FUNCTION public.refresh_space_topic_suggestions()
RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
  topics_prop constant uuid := '806d52bc-27e9-4c91-93c0-57978b093351';
  types_prop  constant uuid := '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1';
  name_prop   constant uuid := 'a126ca53-0c8e-48d5-b888-82c734c38935';
  claim_type  constant uuid := '96f859ef-a1ca-4b22-9372-c86ad58b694b';
  debate_type constant uuid := 'fd51f935-2063-4617-be39-7b672b23364c';
  topic_type  constant uuid := '5ef5a586-0f27-4d8e-8f6c-59ae5b3e89e2';
  written integer;
BEGIN
  DELETE FROM space_topic_suggestions;

  INSERT INTO space_topic_suggestions (space_id, topic_id, name_key, links, rank)
  WITH counted AS (
    SELECT r.space_id, r.to_entity_id AS topic_id, count(DISTINCT r.from_entity_id)::integer AS links
    FROM relations r
    WHERE r.type_id = topics_prop
      AND EXISTS (
        SELECT 1 FROM relations t
        WHERE t.from_entity_id = r.from_entity_id
          AND t.type_id = types_prop
          AND t.to_entity_id IN (claim_type, debate_type)
      )
    GROUP BY r.space_id, r.to_entity_id
  ),
  named AS (
    -- An entity can carry a Name in several spaces; any one serves as its key, chosen
    -- deterministically so a refresh never flips between two spellings.
    SELECT c.*, regexp_replace(lower(btrim(n.text)), '\s+', ' ', 'g') AS name_key
    FROM counted c
    CROSS JOIN LATERAL (
      SELECT v.text FROM values v
      WHERE v.entity_id = c.topic_id AND v.property_id = name_prop AND btrim(coalesce(v.text, '')) <> ''
      ORDER BY v.space_id
      LIMIT 1
    ) n
    WHERE EXISTS (
      SELECT 1 FROM relations t
      WHERE t.from_entity_id = c.topic_id AND t.type_id = types_prop AND t.to_entity_id = topic_type
    )
  ),
  own AS (
    SELECT s.id AS space_id, s.topic_id AS own_topic_id,
           (SELECT regexp_replace(lower(btrim(v.text)), '\s+', ' ', 'g') FROM values v
             WHERE v.entity_id = s.topic_id AND v.property_id = name_prop AND btrim(coalesce(v.text, '')) <> ''
             ORDER BY v.space_id LIMIT 1) AS own_name_key
    FROM spaces s
  ),
  kept AS (
    SELECT n.* FROM named n
    LEFT JOIN own o ON o.space_id = n.space_id
    WHERE n.topic_id IS DISTINCT FROM o.own_topic_id
      AND n.name_key IS DISTINCT FROM o.own_name_key
  ),
  one_per_name AS (
    SELECT DISTINCT ON (space_id, name_key) *
    FROM kept
    ORDER BY space_id, name_key, links DESC, topic_id
  )
  SELECT space_id, topic_id, name_key, links,
         row_number() OVER (PARTITION BY space_id ORDER BY links DESC, topic_id)::integer
  FROM one_per_name;

  GET DIAGNOSTICS written = ROW_COUNT;
  RETURN written;
END $$;
--> statement-breakpoint

-- The onboarding read. One topic per name across the chosen spaces, at its best rank in any of
-- them, ordered by that rank so the chosen spaces interleave (each space's #1, then each #2, …),
-- ties broken by link count. Pages through PostGraphile's `first`/`offset`.
--
-- Also drops the own topic of EVERY chosen space, not just each space's own from its list:
-- someone who picked AI and Health was otherwise offered "AI" third, from Health's content,
-- right after choosing the AI space. Measured on the live DB: 7.6 ms for three spaces.
CREATE OR REPLACE FUNCTION public.topic_suggestions_for_spaces(space_ids uuid[])
RETURNS SETOF public.entities
LANGUAGE sql STABLE PARALLEL SAFE AS $$
  WITH chosen_own AS (
    SELECT regexp_replace(lower(btrim(n.text)), '\s+', ' ', 'g') AS name_key
    FROM spaces sp
    CROSS JOIN LATERAL (
      SELECT v.text FROM values v
      WHERE v.entity_id = sp.topic_id
        AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'
        AND btrim(coalesce(v.text, '')) <> ''
      ORDER BY v.space_id
      LIMIT 1
    ) n
    WHERE sp.id = ANY(space_ids)
  ),
  picked AS (
    SELECT DISTINCT ON (s.name_key) s.topic_id, s.rank, s.links
    FROM space_topic_suggestions s
    WHERE s.space_id = ANY(space_ids)
      AND s.name_key NOT IN (SELECT name_key FROM chosen_own)
    ORDER BY s.name_key, s.rank, s.links DESC, s.topic_id
  )
  SELECT e.*
  FROM picked p
  JOIN entities e ON e.id = p.topic_id
  ORDER BY p.rank, p.links DESC, p.topic_id
$$;
--> statement-breakpoint

COMMENT ON FUNCTION public.topic_suggestions_for_spaces(uuid[]) IS
  E'Topics to suggest after someone picks spaces in onboarding, most linked first, interleaved across the spaces (GEO-3079). Refreshed hourly.';
