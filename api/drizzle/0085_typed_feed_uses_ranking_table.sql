-- GEO-2885 stage 3: point `entities_ranked_for_feed_by_type` at the denormalised
-- `entity_type_ranking` table, so a type-scoped feed stops depending on type size.
--
-- Requires 0084 AND its backfill. The table was populated in production on 2026-09-15
-- (1,311,783 rows / 894 types in 56 s, 0 failures) and verified against an independently
-- derived count with 0 drifted scores. Do not apply this to an environment where the
-- backfill has not run — the function would return nothing.
--
-- WHAT WAS WRONG WITH BOTH PREVIOUS PLANS
--
-- Measured on live data at Explore's window, with every feed predicate applied:
--
--                              ranked walk (0077)   semi-join (0082)   this
--   Debate, 63 qualifying          TIMEOUT              9 ms          2.9-7.9 ms
--   Claim, 334,808                  10 ms          11,405 ms         12.4 ms
--   Person, 17,467               1,617 ms               --            7.4 ms
--   12-type whitelist               16 ms          11,408 ms         43.5 ms
--
-- 0082's semi-join gathers the whole type before ranking, so it collapses on a large
-- type. The walk is the mirror image: it is cheap while it can FIND matches and
-- unbounded when it must PROVE there are none, which is why Debate-only times out. See
-- 0084's header for the live 500 that produces.
--
-- THE SHAPE, AND WHY THE NATURAL PHRASING IS WRONG
--
-- `type_id = ANY(type_ids) ORDER BY ranking_score DESC LIMIT n` is correct and 75x
-- slower: Postgres does one index search per type, reads 56,477 rows and top-N sorts.
-- `= ANY` does not preserve the index ordering. One ordered scan PER TYPE, driven by a
-- LATERAL and capped, reads 701 rows instead. Same family as 0077's `IS NULL OR` and
-- 0082's `IN`-under-`OR`: the planner will not find the good plan from the natural
-- phrasing, and the bad plan looks fine on a dev-sized table.
--
-- DISTINCT ON IS LOAD-BEARING, NOT TIDINESS
--
-- The function this replaces de-duplicated for free: `e.id IN (SELECT ...)` is a
-- semi-join, so an entity carrying two of the requested types came back ONCE. A LATERAL
-- is not a semi-join — it is one walk per type, concatenated — so the same entity comes
-- back once per matching type. With Explore defaulting to News story + Debate + Claim,
-- anything typed both Claim and News story would render as two cards. Verified on live
-- data before writing this: the bare LATERAL returns a two-typed entity twice.
--
-- The predicates must also stay INSIDE the lateral. Applied outside, a per-type limit
-- can be consumed entirely by filtering and the feed silently returns short.
--
-- WHY `max_per_type` EXISTS, AND WHY ITS DEFAULT IS NULL
--
-- The cap is what makes this fast, and this function cannot see the caller's window: it
-- returns SETOF and PostGraphile applies LIMIT/OFFSET outside it. So the cap is an
-- argument, and `LIMIT NULL` is Postgres's own spelling of "no limit" — no branching and
-- no `IS NULL OR` guard of the kind 0077 and 0082 were bitten by. Verified both halves on
-- live data: `LIMIT NULL` returns everything, and a parameterised `LIMIT $1` still
-- terminates the index scan early (66 rows, 4 buffers, 0.064 ms).
--
-- Measured on live data, same rows in every case:
--
--                        old (0082)    uncapped    capped at 66
--   Claim, 334,808         8,646 ms    6,608 ms      **6.5 ms**
--   12-type whitelist     10,399 ms    7,845 ms     **35.4 ms**
--
-- **Read the uncapped column before assuming the default is good enough.** It is NOT the
-- cheap table scan it might look like: the feed predicates live inside the lateral, so
-- uncapped they are evaluated for every one of a large type's rows. (An earlier draft of
-- this header claimed 90-148 ms, measured without the predicates. It was wrong, and it is
-- the kind of number that gets believed later — hence the correction rather than a quiet
-- edit.) Uncapped is a modest improvement on 0082, not a transformation.
--
-- **The default is still NULL — correct rather than fast — deliberately.** A cap that is
-- too small is SILENTLY WRONG: it truncates each type's candidate list, so a deep page
-- returns fewer rows than it should with nothing to signal it. A NULL default means no
-- existing caller can be broken by this migration and none gets slower; the fast path is
-- opt-in. Note the current caller, the debates browse page, queries Debate (63 members),
-- where uncapped is already single-digit milliseconds — the uncapped cost only bites on
-- a large type, which is Explore's case, and Explore's switch passes the cap.
--
-- Pass `first + offset`: the smallest cap that is provably exact, since taking the top N
-- of each type guarantees the global top N is among them. Verified rather than argued —
-- on live data, capped at 66 returns byte-identical results to uncapped for both Claim
-- and the 12-type whitelist.
--
-- Ordering is unchanged — `ranking_score DESC, entity_id DESC`, the same total order as
-- `entity_ranking_scores_ranking_desc_idx` — so cursors stay stable across the switch.

-- DROP FIRST, and drop the EXACT old signature. `CREATE OR REPLACE` cannot change a
-- function's argument list — it creates an OVERLOAD instead. With both the 5-arg and the
-- 6-arg version present, every existing call resolves ambiguously and fails outright:
--
--   ERROR: function public.entities_ranked_for_feed_by_type(uuid[]) is not unique
--
-- which is every typed feed request in production. Found by running this migration
-- against live data before shipping it. 0077 hit the same thing and handled it the same
-- way. Drizzle runs a migration in one transaction, so no concurrent caller observes the
-- window where the function does not exist.
DROP FUNCTION IF EXISTS public.entities_ranked_for_feed_by_type(uuid[], numeric, text, text, uuid[]);
--> statement-breakpoint

CREATE FUNCTION public.entities_ranked_for_feed_by_type(
  type_ids uuid[],
  min_ranking_score numeric DEFAULT NULL,
  created_after text DEFAULT NULL,
  created_before text DEFAULT NULL,
  space_ids uuid[] DEFAULT NULL,
  max_per_type integer DEFAULT NULL
)
RETURNS SETOF public.entities
LANGUAGE sql STABLE PARALLEL SAFE AS $$
  SELECT e.*
  FROM (
    -- One entity per id, whichever type row won. `ranking_score` is a property of the
    -- entity, not of the (entity, type) pair, so the two rows carry the same score and
    -- which one survives cannot change the order.
    SELECT DISTINCT ON (s.entity_id) s.entity_id, s.ranking_score
    FROM unnest(type_ids) AS t(tid)
    CROSS JOIN LATERAL (
      SELECT etr.entity_id, etr.ranking_score
      FROM public.entity_type_ranking etr
      WHERE etr.type_id = t.tid
        AND (min_ranking_score IS NULL OR etr.ranking_score >= min_ranking_score)
        -- Everything below is the same predicate set as the function this replaces,
        -- moved inside the lateral so the per-type cap counts only rows that survive.
        AND EXISTS (
          SELECT 1 FROM public.entities e2
          WHERE e2.id = etr.entity_id
            AND (created_after  IS NULL OR e2.created_at >  created_after)
            AND (created_before IS NULL OR e2.created_at <= created_before)
        )
        AND EXISTS (
          SELECT 1 FROM public.values v
          WHERE v.entity_id = etr.entity_id
            AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid
            AND v.text IS NOT NULL
            AND length(trim(v.text)) > 0
            AND (space_ids IS NULL OR v.space_id = ANY(space_ids))
        )
        AND NOT EXISTS (
          SELECT 1 FROM public.entity_feed_blocklist b WHERE b.entity_id = etr.entity_id
        )
        -- System entities are infrastructure, not content (0076).
        AND NOT EXISTS (
          SELECT 1 FROM public.relations r
          WHERE r.from_entity_id = etr.entity_id
            AND r.type_id = '88b3d6ad-288c-529c-a212-0e1c24819185'::uuid  -- System Type
        )
        -- Editorial exclusions still win over a caller's narrowing request.
        AND NOT EXISTS (
          SELECT 1
          FROM public.relations r
          JOIN public.entity_type_exclusions x ON x.type_id = r.to_entity_id
          WHERE r.from_entity_id = etr.entity_id
            AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid  -- TYPES
        )
      ORDER BY etr.ranking_score DESC, etr.entity_id DESC
      LIMIT max_per_type   -- NULL means unbounded; see the header
    ) s
    ORDER BY s.entity_id, s.ranking_score DESC
  ) d
  JOIN public.entities e ON e.id = d.entity_id
  ORDER BY d.ranking_score DESC, d.entity_id DESC;
$$;
