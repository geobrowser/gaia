-- Explore feed: never serve system entities.
--
-- Preston's instruction was "all system entities can be excluded (space system
-- entity, proposal system entity, etc.)". That is a category, not a type name, so it
-- cannot be expressed in `entity_type_exclusions`, which is keyed on user-authored
-- Type relations.
--
-- The right marker is the System Type relation, 88b3d6ad-288c-529c-a212-0e1c24819185
-- (SYSTEM_TYPES_RELATION_TYPE_ID). Per 0065, it is system-minted and the indexer
-- always drops user attempts to author it, so unlike a type name it cannot be
-- forged — someone minting a type entity named "Space" does not become a system
-- entity, and a real system entity cannot shed the marker.
--
-- Measured on production 2026-08-13: 28,744 entities carry it, by system type —
--   System 28,744 · Proposal 27,508 · Space 1,236 · EOA Space 822 · DAO Space 414
--
-- Note this does NOT subsume the name-keyed exclusions already configured:
--   * Payout has ZERO system-typed entities, so it still needs its own row.
--   * The 1,146 user-typed "Space" entities are a DIFFERENT set from the 1,236
--     system-typed ones — no overlap. Both exclusions are required.
--
-- No new index: `relations_type_from_to_idx` already serves a (type_id,
-- from_entity_id) probe, which is the same shape the excluded-type check uses.

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
    -- Unrenderable entities are never candidates (0075).
    AND EXISTS (
      SELECT 1 FROM public.values v
      WHERE v.entity_id = e.id
        AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid
        AND v.text IS NOT NULL
        AND length(trim(v.text)) > 0
    )
    -- Never serve blocked entities, whatever they score.
    AND NOT EXISTS (SELECT 1 FROM public.entity_feed_blocklist b WHERE b.entity_id = e.id)
    -- System entities are infrastructure, not content. Keyed on the unforgeable
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
