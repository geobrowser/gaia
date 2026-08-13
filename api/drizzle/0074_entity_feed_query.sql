-- Explore feed "Best" — Phase A query path.
--
-- Companion to 0073, which stores the scores. This adds the read side:
--   * `entities_ranked_for_feed(...)` — candidate generation, exposed to GraphQL as
--     a connection so cursor pagination comes for free.
--   * computed columns so the score and its inputs are selectable on Entity, which
--     the PRD asks for so a surprising ranking can be explained.
--   * `entity_feed_blocklist` — the enforcement point for "must never be served".
--
-- On the blocklist: this schema has NO entity-level moderation state. There is no
-- flagged / deleted / moderated table anywhere in `public` (checked). The PRD
-- requires that such entities never be served regardless of score, so the filter
-- lives here and is applied unconditionally; it is simply empty until something
-- upstream populates it. Building the seam now means the feed does not have to be
-- revisited when moderation lands, and an empty table is honest about the gap in a
-- way that omitting the filter would not be.

CREATE TABLE IF NOT EXISTS "entity_feed_blocklist" (
  "entity_id" uuid PRIMARY KEY,
  "reason" text,
  "created_at" timestamp with time zone NOT NULL DEFAULT now()
);
--> statement-breakpoint

-- ---------------------------------------------------------------------------
-- Candidate generation.
--
-- Returns SETOF entities so PostGraphile exposes it as a connection with cursor
-- pagination, matching the shape the frontend already consumes.
--
-- Why a function rather than only an orderBy on entitiesConnection: the exclusions
-- are not optional. Routing the feed through one function means a caller cannot
-- forget to apply them, whereas an orderBy leaves filtering to each call site.
-- An orderBy is added alongside for flexibility, but this is the feed's entry point.
--
-- `created_after` / `created_before` are text epoch seconds, matching
-- entities.created_at. Comparison is lexicographic, which equals numeric ordering
-- while every value is 10 digits — true until the year 2286. Passing them as text
-- keeps `entities_created_at_id_idx` usable; casting to bigint here would not.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.entities_ranked_for_feed(
  min_ranking_score numeric DEFAULT NULL,
  created_after text DEFAULT NULL,
  created_before text DEFAULT NULL
)
RETURNS SETOF public.entities
LANGUAGE sql STABLE PARALLEL SAFE AS $$
  SELECT e.*
  FROM public.entities e
  JOIN public.entity_ranking_scores rs ON rs.entity_id = e.id
  WHERE (min_ranking_score IS NULL OR rs.ranking_score >= min_ranking_score)
    AND (created_after   IS NULL OR e.created_at >  created_after)
    AND (created_before  IS NULL OR e.created_at <= created_before)
    -- Never serve blocked entities, whatever they score.
    AND NOT EXISTS (SELECT 1 FROM public.entity_feed_blocklist b WHERE b.entity_id = e.id)
    -- Excluded types are filtered at candidate generation rather than via a zero
    -- weight, so they never consume candidate slots.
    AND NOT EXISTS (
      SELECT 1
      FROM public.relations r
      JOIN public.entity_type_exclusions x ON x.type_id = r.to_entity_id
      WHERE r.from_entity_id = e.id
        AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid  -- TYPES
    )
  -- entity_id breaks ties so the ordering is total and cursor pagination cannot
  -- skip or duplicate rows between equally-scored entities.
  ORDER BY rs.ranking_score DESC, rs.entity_id DESC;
$$;
--> statement-breakpoint

-- ---------------------------------------------------------------------------
-- Computed columns. PostGraphile surfaces `entities_<name>(entities)` as field
-- `<name>` on Entity, so these become entity.rankingScore / .qualityScore /
-- .intrinsicScore / .upvotes / .downvotes.
--
-- Each is a primary-key lookup on entity_ranking_scores, so the cost is per
-- returned row (a page of ~20), not per candidate scanned.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.entities_ranking_score(e public.entities)
RETURNS numeric LANGUAGE sql STABLE PARALLEL SAFE AS $$
  SELECT rs.ranking_score FROM public.entity_ranking_scores rs WHERE rs.entity_id = e.id;
$$;
--> statement-breakpoint
CREATE OR REPLACE FUNCTION public.entities_quality_score(e public.entities)
RETURNS numeric LANGUAGE sql STABLE PARALLEL SAFE AS $$
  SELECT rs.quality_score FROM public.entity_ranking_scores rs WHERE rs.entity_id = e.id;
$$;
--> statement-breakpoint
CREATE OR REPLACE FUNCTION public.entities_intrinsic_score(e public.entities)
RETURNS numeric LANGUAGE sql STABLE PARALLEL SAFE AS $$
  SELECT rs.intrinsic_score FROM public.entity_ranking_scores rs WHERE rs.entity_id = e.id;
$$;
--> statement-breakpoint
-- Named upvotes/downvotes because that is what the feed's curation axis means to a
-- client, and it matches the vocabulary the web client already uses. The stored
-- columns are positive/negative because the same tallies serve other vote kinds.
CREATE OR REPLACE FUNCTION public.entities_upvotes(e public.entities)
RETURNS bigint LANGUAGE sql STABLE PARALLEL SAFE AS $$
  SELECT rs.positive FROM public.entity_ranking_scores rs WHERE rs.entity_id = e.id;
$$;
--> statement-breakpoint
CREATE OR REPLACE FUNCTION public.entities_downvotes(e public.entities)
RETURNS bigint LANGUAGE sql STABLE PARALLEL SAFE AS $$
  SELECT rs.negative FROM public.entity_ranking_scores rs WHERE rs.entity_id = e.id;
$$;
