-- Explore "Best": a type-scoped sibling of `entities_ranked_for_feed` that can
-- actually use a semi-join.
--
-- ADDITIVE. The existing function and its field are untouched, so nothing changes
-- until a caller asks for the new one.
--
-- WHY A SECOND FUNCTION RATHER THAN FIXING THE FIRST (GEO-2793)
--
-- `entities_ranked_for_feed` walks `entity_ranking_scores_ranking_desc_idx` in score
-- order and applies `type_ids` as a per-candidate filter. `entity_ranking_scores` is
-- 48,886,636 rows while only ~1.46M entities carry a Types relation at all, so a
-- type-scoped walk wades through tens of millions of untyped rows. Cost is how deep
-- the walk must go, which is why it tracks type *rarity* rather than the length of
-- the list. At the Explore window size (`first: 66`), measured on live data:
--
--   whitelist (11 types) + 5 spaces   5,750-6,133 ms   <- the real Explore query
--   Person alone (17,467 members)     1,635-2,810 ms
--   Debate alone (65 members)         >120,000 ms, cancelled 3 of 3
--
-- The obvious in-place fix does not work. 0077's header says a NULL argument "folds
-- the guard away at plan time"; for a subquery predicate that is false. An `IN (...)`
-- nested under an `OR` cannot be converted to a semi-join, and the folding happens
-- too late to rescue it:
--
--   type_ids IS NULL OR e.id IN (SELECT ...)   ->  statement timeout
--   e.id IN (SELECT ...)          (no guard)   ->  11 ms
--
-- Same body otherwise. Rewriting the guard as `UNION ALL` prunes the dead branch and
-- fixes the typed path, but the function returns `SETOF entities`, so ordering by
-- ranking_score needs a wrapper that re-joins the scores — and that destroys the
-- LIMIT pushdown the untyped path depends on (33 ms -> 16-17 s). One inlined
-- `LANGUAGE sql` function cannot hold both plans.
--
-- Hence: keep the untyped walk where it is good, and give the typed case its own
-- function with no guard, so `IN` is a real semi-join. `type_ids` is NOT NULL here —
-- callers with no type filter must keep using `entities_ranked_for_feed`.
--
--   whitelist (11 types) + 5 spaces     828-1,058 ms   (was 5,750-6,133)
--   Person alone                          263-266 ms   (was 1,635-2,810)
--   Debate alone                           11-12 ms    (was a timeout)
--
-- Same rows and same order as the old path — verified equal on live data, identical
-- id sets with 0 rows exclusive to either side. `ORDER BY rs.ranking_score DESC,
-- rs.entity_id DESC` is unchanged, and entity_id makes it a strict total order.
--
-- KNOWN CEILING — READ BEFORE WIDENING THE TYPE WHITELIST
--
-- A semi-join inverts which case is cheap. For a type with hundreds of thousands of
-- members the old walk is better, because matches are dense enough that it stops
-- almost at once, while the semi-join gathers the whole set:
--
--   type 96f859ef (337,258 members)   walk 6-8 ms   ->   semi-join 8,445-21,240 ms
--
-- Nothing the app can currently request is close: the largest in
-- `EXPLORE_ENTITY_TYPES` is Person at 17,467, and the whole 11-type whitelist is
-- 50,914 distinct entities. If a type that large is ever added to the feed, this
-- needs a denormalised `(type_id, ranking_score DESC, entity_id DESC)` index over the
-- ~1.46M typed entities instead, which would make both ends an ordered walk that
-- terminates at `first` and would remove the ceiling entirely.
--
-- Stays `LANGUAGE sql` so it keeps being inlined; the non-type predicates keep the
-- `arg IS NULL OR ...` form, which does fold correctly for plain comparisons.

CREATE OR REPLACE FUNCTION public.entities_ranked_for_feed_by_type(
  type_ids uuid[],
  min_ranking_score numeric DEFAULT NULL,
  created_after text DEFAULT NULL,
  created_before text DEFAULT NULL,
  space_ids uuid[] DEFAULT NULL
)
RETURNS SETOF public.entities
LANGUAGE sql STABLE PARALLEL SAFE AS $$
  SELECT e.*
  FROM public.entities e
  JOIN public.entity_ranking_scores rs ON rs.entity_id = e.id
  -- No `type_ids IS NULL OR` guard: that is what turns this back into a per-row
  -- filter. The argument is declared NOT NULL-in-practice by contract.
  WHERE e.id IN (
        SELECT r.from_entity_id
        FROM public.relations r
        WHERE r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid  -- TYPES
          AND r.to_entity_id = ANY(type_ids)
      )
    AND (min_ranking_score IS NULL OR rs.ranking_score >= min_ranking_score)
    AND (created_after   IS NULL OR e.created_at >  created_after)
    AND (created_before  IS NULL OR e.created_at <= created_before)
    -- Unrenderable entities are never candidates (0075), AND — when `space_ids` is
    -- given — the name must exist in one of those spaces. Identical to 0077.
    AND EXISTS (
      SELECT 1 FROM public.values v
      WHERE v.entity_id = e.id
        AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid
        AND v.text IS NOT NULL
        AND length(trim(v.text)) > 0
        AND (space_ids IS NULL OR v.space_id = ANY(space_ids))
    )
    -- Never serve blocked entities, whatever they score.
    AND NOT EXISTS (SELECT 1 FROM public.entity_feed_blocklist b WHERE b.entity_id = e.id)
    -- System entities are infrastructure, not content (0076).
    AND NOT EXISTS (
      SELECT 1 FROM public.relations r
      WHERE r.from_entity_id = e.id
        AND r.type_id = '88b3d6ad-288c-529c-a212-0e1c24819185'::uuid  -- System Type
    )
    -- Editorial exclusions still win over a caller's narrowing request.
    AND NOT EXISTS (
      SELECT 1
      FROM public.relations r
      JOIN public.entity_type_exclusions x ON x.type_id = r.to_entity_id
      WHERE r.from_entity_id = e.id
        AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid  -- TYPES
    )
  ORDER BY rs.ranking_score DESC, rs.entity_id DESC;
$$;
