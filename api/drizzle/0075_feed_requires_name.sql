-- Explore feed: never serve an entity with no name.
--
-- Measured on production 2026-08-13, and this is the single largest defect in the
-- Phase A feed by a wide margin:
--
--   scored entities                     48,884,134
--   of those with NO name at all        47,808,504   (97.8%)
--   nameless entities in the top 2,000       1,455   (73%)
--   first nameless result                 rank 497
--
-- So from roughly rank 500 onward the feed was mostly blank rows. A nameless
-- entity has nothing to render — it surfaces as a raw uuid in the UI (the symptom
-- reported on GEO-2548 for 660ef494c6d346a0a630b7cf449bfc9e) and, if picked from a
-- collection dropdown, becomes a dangling reference in published content.
--
-- Root cause of the population: 99.4% of untyped entities are empty shells — bare
-- `entities` rows with no values, no relations, and referenced by nothing. Sampling
-- 20,000 of them found 101 with a name and 0 referenced by any relation. They are
-- created as identifiers and never filled in.
--
-- Why this belongs at candidate generation rather than in the score: an entity with
-- no name is not "low quality", it is unrenderable. Demoting it by weight would
-- still surface it once nothing fresher existed. Type weights cannot fix this
-- either — the population is untyped by definition, so no type config reaches it.
--
-- The name property is a126ca53-0c8e-48d5-b888-82c734c38935 (SystemIds.NAME_PROPERTY),
-- the same id `entityComputedTextFilterPlugin` uses for EntityFilter.name.
--
-- NOTE: this filter is intentionally "has a name value", not "has any value". A
-- description without a name still renders as a raw id, so name presence is the
-- property that actually determines whether a row is displayable.

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
    -- Unrenderable entities are never candidates. See the header: this removes
    -- 97.8% of the scored corpus, all of it content-free.
    AND EXISTS (
      SELECT 1 FROM public.values v
      WHERE v.entity_id = e.id
        AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid
        AND v.text IS NOT NULL
        AND length(trim(v.text)) > 0
    )
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
  ORDER BY rs.ranking_score DESC, rs.entity_id DESC;
$$;
--> statement-breakpoint

-- Supports the name-presence check as an index-only probe rather than a heap fetch
-- per candidate. Partial, because only non-empty name values are ever probed.
CREATE INDEX IF NOT EXISTS "values_name_entity_idx"
  ON "values" ("entity_id")
  WHERE property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid
    AND text IS NOT NULL;
