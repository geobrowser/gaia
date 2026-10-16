-- GraphQL `search` / `searchConnection`: let the caller supply the candidate ids.
--
-- WHY. public.search is a pg_trgm similarity scan over every name and description value: about
-- 9-10 s per call whatever `first` is. The REST /search route answers the same question from
-- OpenSearch in about 0.5 s. The API's graphqlSearchOpenSearchPlugin asks OpenSearch for the ids and
-- passes them in here, so the result is still real `entities` rows: nested selections, the
-- connection `filter`, `offset`, `first` and cursor pagination all keep working unchanged.
--
-- WHAT. A fourth argument, `ids uuid[] DEFAULT NULL`.
--   ids IS NOT NULL  return those entities in the order given (OpenSearch rank). The trigram
--                    logic does not run, and `query`, `space_id` and `similarity_threshold` are
--                    ignored: the id list is already the answer.
--   ids IS NULL      the legacy trigram body, unchanged. This is the kill switch (set
--                    GRAPHQL_SEARCH_OPENSEARCH=0) and the fallback when OpenSearch is down.
-- Pods still running the old build call this with three arguments; they resolve through the default.
--
-- The function is dropped and recreated rather than replaced: CREATE OR REPLACE cannot change an
-- argument list, and the old three-argument overload must not survive beside the new one (a call
-- with three arguments would then be ambiguous).
--
-- A single SQL function, not plpgsql, so the planner keeps seeing a plain query. The two branches
-- are guarded by `ids IS [NOT] NULL`, which the planner resolves to a one-time filter, so the
-- branch not taken does not execute.

DROP FUNCTION IF EXISTS public.search(text, uuid, double precision);
--> statement-breakpoint
CREATE FUNCTION public.search(
  query TEXT,
  space_id UUID DEFAULT NULL,
  similarity_threshold FLOAT DEFAULT 0.3,
  ids UUID[] DEFAULT NULL
) RETURNS SETOF public.entities AS $$
  WITH search_values AS (
    SELECT
      v.entity_id,
      CASE
        WHEN v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935' THEN similarity(v."text", query) * 2.0  -- Name property
        WHEN v.property_id = '9b1f76ff-9711-404c-861e-59dc3fa7d037' THEN similarity(v."text", query) * 1.5  -- Description property
      END AS sim_score
    FROM
      "values" v
    WHERE
      ids IS NULL
      AND v."text" % query
      AND similarity(v."text", query) >= similarity_threshold
      AND (space_id IS NULL OR v.space_id = space_id)
      AND (
        v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935' OR  -- Name property
        v.property_id = '9b1f76ff-9711-404c-861e-59dc3fa7d037'      -- Description property
      )
  ),
  ranked_entities AS (
    SELECT
      sv.entity_id,
      MAX(sv.sim_score) AS max_score
    FROM
      search_values sv
    GROUP BY
      sv.entity_id
  ),
  candidates AS (
    -- Supplied ids: rank is the position in the array. A duplicated id keeps its first position.
    SELECT u.id AS entity_id, MIN(u.ord)::float8 AS rank_key
    FROM unnest(ids) WITH ORDINALITY AS u(id, ord)
    WHERE ids IS NOT NULL
    GROUP BY u.id
    UNION ALL
    -- Legacy: best similarity first, so the key is the negated score.
    SELECT re.entity_id, -re.max_score::float8
    FROM ranked_entities re
    WHERE ids IS NULL
  )
  SELECT e.*
  FROM
    candidates c
    JOIN entities e ON e.id = c.entity_id
  ORDER BY
    c.rank_key, e.id;
$$ LANGUAGE sql STABLE;
