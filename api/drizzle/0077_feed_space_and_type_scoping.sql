-- Explore feed: accept space and type scoping, so "Best" can serve the same
-- surfaces "New" and "Top" already serve.
--
-- WHY THIS IS NEEDED
--
-- `fetchExploreFeed` (geogenesis apps/web/core/explore/fetch-explore-feed.ts) is
-- space-scoped by construction: it derives `baseIds` from the browse sidebar,
-- narrows to one space when the UI's space filter is set, and returns an empty feed
-- when the set is empty. Both existing sorts pass that set to the server —
-- "New" via `entitiesConnection(spaceIds:)` and "Top" via
-- `entitiesOrderedByPropertyConnection(spaceIds:, typeIds:)`.
--
-- `entities_ranked_for_feed` took neither, so a "Best" sort built on it would have
-- silently ignored the space filter — the control would still be on screen and do
-- nothing on that one tab. This adds the two parameters rather than leaving the
-- frontend to fake them.
--
-- WHY NOT PUT SPACE SCOPING IN `filter`
--
-- `EntityFilter.spaceIds` exists and would have needed no migration, but it is
-- backed by `entities_space_ids(entities)` — a computed column (0004), not a stored
-- array. Filtering on it costs a function call per candidate row and cannot be
-- index-driven. `entities_ordered_by_property` reached the same conclusion in 0060
-- and scopes on `values.space_id` directly; this follows it.
--
-- WHY THIS STAYS `LANGUAGE sql` AND DOES NOT COPY 0060's DYNAMIC SQL
--
-- This is the load-bearing decision, and 0060's own header warns about the failure
-- mode it avoids: "LIMIT/OFFSET are applied by PostGraphile outside this function,
-- so PG materializes the full ordered set before paginating." 0060 accepts that
-- because its type-prefiltered set is small.
--
-- The feed's candidate set is not small — 960,495 rows as of 2026-08-13. It is fast
-- (~0.3s, and 0.32s at offset 10,000) only because a single-statement `LANGUAGE sql`
-- function is INLINED into the calling query, which lets PostGraphile's LIMIT push
-- down into an index-ordered walk of `entity_ranking_scores_ranking_desc_idx` that
-- terminates early. Rewriting this as plpgsql with `RETURN QUERY EXECUTE` would
-- defeat inlining and sort ~1M rows on every request.
--
-- So the predicates are added as `arg IS NULL OR ...` guards, which 0060 explicitly
-- avoids. That is the correct tradeoff *here*: because the function is inlined, the
-- arguments are substituted into the body before planning, so a NULL argument folds
-- the guard away at plan time instead of being evaluated per row. The guard costs
-- nothing that dynamic SQL would save, and dynamic SQL would cost the inlining.
--
-- MEASURED CAVEAT (pre-existing, not introduced here)
--
-- Requesting `filter` + `totalCount` + `edges` together on this connection exceeds
-- the statement timeout: totalCount forces a full scan of the filtered set while
-- edges walks it again, and a low-selectivity filter (e.g. ILIKE) makes each pass
-- deep. Each of the three alone is fine. The Explore documents never request
-- top-level `totalCount` (their only `totalCount` is nested under `backlinks`, for
-- comment counts), so the feed path is unaffected — but callers combining a broad
-- filter with totalCount should expect it.

-- Adding parameters with defaults creates a new signature rather than replacing the
-- old one, which would leave two `entities_ranked_for_feed` functions for PostGraphile
-- to choose between. Drop the 3-arg form explicitly, as 0060 does for its own
-- superseded signatures.
DROP FUNCTION IF EXISTS public.entities_ranked_for_feed(numeric, text, text);
--> statement-breakpoint

CREATE OR REPLACE FUNCTION public.entities_ranked_for_feed(
  min_ranking_score numeric DEFAULT NULL,
  created_after text DEFAULT NULL,
  created_before text DEFAULT NULL,
  space_ids uuid[] DEFAULT NULL,
  type_ids uuid[] DEFAULT NULL
)
RETURNS SETOF public.entities
LANGUAGE sql STABLE PARALLEL SAFE AS $$
  SELECT e.*
  FROM public.entities e
  JOIN public.entity_ranking_scores rs ON rs.entity_id = e.id
  WHERE (min_ranking_score IS NULL OR rs.ranking_score >= min_ranking_score)
    AND (created_after   IS NULL OR e.created_at >  created_after)
    AND (created_before  IS NULL OR e.created_at <= created_before)
    -- Unrenderable entities are never candidates (0075), AND — when `space_ids` is
    -- given — the name must exist in one of those spaces.
    --
    -- Space scoping is folded into this probe rather than added as a separate EXISTS
    -- because the name value already carries the space_id, so one index probe answers
    -- both questions. It also reproduces exactly what the frontend's `requireName`
    -- filter already asks for today:
    --   values: { some: { spaceId: { in: spaceIds },
    --                     propertyId: { is: NAME }, text: { isNull: false } } }
    -- i.e. "has a usable name in a space I'm looking at", not the weaker "has a name
    -- somewhere, and is present in one of these spaces". An entity named only in a
    -- space the viewer is not browsing would render as a raw uuid, which is the whole
    -- defect 0075 exists to prevent.
    AND EXISTS (
      SELECT 1 FROM public.values v
      WHERE v.entity_id = e.id
        AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid
        AND v.text IS NOT NULL
        AND length(trim(v.text)) > 0
        AND (space_ids IS NULL OR v.space_id = ANY(space_ids))
    )
    -- Optional type restriction. OR semantics across the set, matching 0060's
    -- `type_ids`. This is a positive filter supplied by the caller and is independent
    -- of `entity_type_exclusions` below, which is global editorial config — an
    -- excluded type stays excluded even if a caller names it here, since exclusion is
    -- an invariant of the feed and this is a narrowing request.
    AND (
      type_ids IS NULL OR EXISTS (
        SELECT 1 FROM public.relations r
        WHERE r.from_entity_id = e.id
          AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid  -- TYPES
          AND r.to_entity_id = ANY(type_ids)
      )
    )
    -- Never serve blocked entities, whatever they score.
    AND NOT EXISTS (SELECT 1 FROM public.entity_feed_blocklist b WHERE b.entity_id = e.id)
    -- System entities are infrastructure, not content (0076). Keyed on the unforgeable
    -- System Type relation rather than on type names.
    AND NOT EXISTS (
      SELECT 1 FROM public.relations r
      WHERE r.from_entity_id = e.id
        AND r.type_id = '88b3d6ad-288c-529c-a212-0e1c24819185'::uuid  -- System Type
    )
    -- Excluded types are filtered at candidate generation rather than via a zero
    -- weight, so they never consume candidate slots.
    AND NOT EXISTS (
      SELECT 1
      FROM public.relations r
      JOIN public.entity_type_exclusions x ON x.type_id = r.to_entity_id
      WHERE r.from_entity_id = e.id
        AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid  -- TYPES
    )
  ORDER BY rs.ranking_score DESC, rs.entity_id DESC;
$$;
--> statement-breakpoint

-- The name probe now also filters on space_id. `values_name_entity_idx` (0075) is
-- keyed on entity_id alone with the property/text predicate, so space_id came back
-- from the heap. Including it makes the probe index-only again for the space-scoped
-- path, which is the path the Explore UI actually takes.
CREATE INDEX IF NOT EXISTS "values_name_entity_space_idx"
  ON "values" ("entity_id", "space_id")
  WHERE property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid
    AND text IS NOT NULL;
