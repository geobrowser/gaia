-- GEO-2928: add voted_by / voted_by_kinds to entities_ordered_by_property.
--
-- GEO-2913 (gaia #938) put `votedBy` on `entities` / `entitiesConnection`, which
-- covers Explore's New sort. Top runs on a different connection —
-- `entitiesOrderedByPropertyConnection`, backed by this function — so a person's
-- Positions tab can filter by type and space but cannot sort by Top.
--
-- Why this goes in the function rather than in a plugin
-- -----------------------------------------------------
-- A plugin could add the same semi-join to the OUTER query over the function's
-- result, the way entityVotedByFilterPlugin does for `entities`. It would return
-- the right rows and it would be less code. It would also be much slower in the
-- one case that matters.
--
-- Pagination happens outside this function (see 0060), so the whole result set
-- is built and ordered before any LIMIT applies. With include_without_value the
-- candidate set is "every entity of these types in these spaces" — the comment
-- in 0069 calls out that an unbounded candidate scan could be very large. A
-- filter applied outside would let the function enumerate that whole population
-- and then throw away all but the caller's ~200 voted rows.
--
-- Pushed inside, the vote set *bounds* the scan instead: both the scored and the
-- value-less branches start from "entities this user voted on", which on the
-- reference account is 208 rows rather than the full Claim population.
--
-- That also lets the two guards relax, which is the difference between the
-- feature working and not:
--
--   * include_without_value previously required space_ids, because space_ids was
--     the only thing bounding the scan. voted_by bounds it at least as tightly,
--     so either will now do. The front end hit this exact error on
--     `space_ids is required when include_without_value is true`.
--   * the value-less branch previously required type_ids, because the TYPES
--     relation was the only cheap candidate set. With voted_by the candidates
--     are the voted entities themselves, which is cheaper still.
--
-- Vote kinds: 0 curation, 1 stance, 2 veracity. Stance and veracity together are
-- what "a position on a claim" means; counting unfiltered overstates a person's
-- positions roughly threefold (567 rows against a true 192 on the reference
-- account). Deliberately not defaulted here, matching #938 — this function does
-- not decide what a caller means by a position.
--
-- Index: user_votes is keyed on
-- (user_id, object_id, object_type, space_id, vote_kind) — the primary key from
-- 0086 — so user_id is the leading column and vote_kind is in the same index.
-- No new index is required.

-- Drop every prior signature before recreating. Postgres treats a differing
-- parameter list as an overload rather than a replacement, and two overloads
-- would leave postgraphile to pick one — which is how a field silently changes
-- shape. 0069's signature is the live one; the rest are listed so this migration
-- is safe to run against a database at any earlier point.
DROP FUNCTION IF EXISTS public.entities_ordered_by_property(uuid, uuid, sort_order, text);
--> statement-breakpoint
DROP FUNCTION IF EXISTS public.entities_ordered_by_property(uuid, uuid, sort_order, text, uuid);
--> statement-breakpoint
DROP FUNCTION IF EXISTS public.entities_ordered_by_property(uuid, uuid, sort_order, text, uuid[]);
--> statement-breakpoint
DROP FUNCTION IF EXISTS public.entities_ordered_by_property(uuid, uuid[], sort_order, text);
--> statement-breakpoint
DROP FUNCTION IF EXISTS public.entities_ordered_by_property(uuid, uuid[], sort_order, text, uuid);
--> statement-breakpoint
DROP FUNCTION IF EXISTS public.entities_ordered_by_property(uuid, uuid[], sort_order, text, uuid[]);
--> statement-breakpoint
DROP FUNCTION IF EXISTS public.entities_ordered_by_property(uuid, uuid[], sort_order, text, uuid[], boolean);
--> statement-breakpoint

CREATE OR REPLACE FUNCTION public.entities_ordered_by_property(
  property_id uuid,
  space_ids uuid[] DEFAULT NULL,
  sort_direction sort_order DEFAULT 'ASC',
  data_type text DEFAULT NULL,
  type_ids uuid[] DEFAULT NULL,
  include_without_value boolean DEFAULT false,
  voted_by uuid DEFAULT NULL,
  voted_by_kinds smallint[] DEFAULT NULL
)
RETURNS SETOF entities AS $fn$
DECLARE
  types_rel_id  constant uuid := '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1';  -- TYPES relation: links an entity to its type entity
  resolved_type   text;
  sort_expr       text;
  null_pred       text;
  null_cast       text;
  is_numeric      boolean := false;
  dir             text;
  space_pred      text := '';
  type_pred       text := '';
  rel_space_pred  text := '';
  voted_subquery  text := '';
  voted_value_pred text := '';
  voted_rel_pred  text := '';
  candidate_pred  text;
  candidate_src   text;
  order_expr      text;
  with_unscored   boolean;
  sql             text;
BEGIN
  -- include_without_value needs the candidate scan bounded by SOMETHING, because
  -- the value-less branch enumerates a population and the whole set is
  -- ordered/materialized before PostGraphile applies LIMIT/OFFSET (pagination
  -- happens outside this function, see 0060). space_ids bounds it; so does
  -- voted_by, which is typically far tighter. COALESCE so an explicit NULL flag
  -- behaves like false (no guard).
  IF COALESCE(include_without_value, false)
     AND (space_ids IS NULL OR cardinality(space_ids) = 0)
     AND voted_by IS NULL THEN
    RAISE EXCEPTION 'space_ids or voted_by is required when include_without_value is true'
      USING ERRCODE = '22023';
  END IF;

  IF data_type IS NOT NULL THEN
    resolved_type := lower(data_type);
  ELSE
    SELECT lower(v."text")
      INTO resolved_type
      FROM relations r
      JOIN "values" v ON v.entity_id = r.to_entity_id
     WHERE r.from_entity_id = entities_ordered_by_property.property_id
       AND r.type_id    = '6d29d578-49bb-4959-baf7-2cc696b1671a'
       AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'
     ORDER BY r.id
     LIMIT 1;
  END IF;

  -- null_cast is the SQL type of sort_expr; the value-less branch selects NULL::null_cast so
  -- the UNION ALL with the scored branch has matching column types.
  CASE resolved_type
    WHEN 'text'     THEN sort_expr := 'left(v."text", 1024)'; null_pred := 'v."text" IS NOT NULL AND length(btrim(v."text")) > 0'; null_cast := 'text';
    WHEN 'integer'  THEN sort_expr := 'v.integer';            null_pred := 'v.integer IS NOT NULL'; is_numeric := true; null_cast := 'bigint';
    WHEN 'float'    THEN sort_expr := 'v.float';              null_pred := 'v.float IS NOT NULL'; is_numeric := true; null_cast := 'double precision';
    WHEN 'decimal'  THEN sort_expr := 'v."decimal"';          null_pred := 'v."decimal" IS NOT NULL'; is_numeric := true; null_cast := 'numeric';
    WHEN 'boolean'  THEN sort_expr := 'v.boolean';            null_pred := 'v.boolean IS NOT NULL'; null_cast := 'boolean';
    WHEN 'date'     THEN sort_expr := 'v.date';               null_pred := 'v.date IS NOT NULL AND length(btrim(v.date)) > 0'; null_cast := 'text';
    WHEN 'time'     THEN sort_expr := 'v."time"';             null_pred := 'v."time" IS NOT NULL AND length(btrim(v."time")) > 0'; null_cast := 'text';
    WHEN 'datetime' THEN sort_expr := 'v.datetime';           null_pred := 'v.datetime IS NOT NULL AND length(btrim(v.datetime)) > 0'; null_cast := 'text';
    WHEN 'point'    THEN sort_expr := 'v.point';              null_pred := 'v.point IS NOT NULL AND length(btrim(v.point)) > 0'; null_cast := 'text';
    ELSE
      RETURN;
  END CASE;

  dir := CASE WHEN sort_direction::text = 'DESC' THEN 'DESC' ELSE 'ASC' END;

  IF space_ids IS NOT NULL AND cardinality(space_ids) > 0 THEN
    space_pred     := format(' AND v.space_id = ANY(%L::uuid[])', space_ids);
    rel_space_pred := format(' AND r.space_id = ANY(%L::uuid[])', space_ids);
  END IF;

  IF type_ids IS NOT NULL AND cardinality(type_ids) > 0 THEN
    type_pred := format(
      ' AND EXISTS (SELECT 1 FROM relations r WHERE r.from_entity_id = v.entity_id AND r.type_id = %L AND r.to_entity_id = ANY(%L::uuid[]) AND r.space_id = v.space_id)',
      types_rel_id, type_ids);
  END IF;

  -- A plain semi-join over one table, deliberately NOT `EXISTS (...) OR EXISTS
  -- (...)`: that pair cannot be planned as a semi-join and cost 22.5 s per
  -- request on a sparse space in GEO-2882.
  --
  -- object_type is not constrained. Every row measured carries 0, and joining
  -- against entity ids already restricts the result to real entities, so a
  -- predicate on it could never change the answer.
  IF voted_by IS NOT NULL THEN
    voted_subquery := format(
      'SELECT uv.object_id FROM user_votes uv WHERE uv.user_id = %L::uuid', voted_by);
    IF voted_by_kinds IS NOT NULL AND cardinality(voted_by_kinds) > 0 THEN
      voted_subquery := voted_subquery
        || format(' AND uv.vote_kind = ANY(%L::smallint[])', voted_by_kinds);
    END IF;
    voted_value_pred := ' AND v.entity_id IN (' || voted_subquery || ')';
    voted_rel_pred   := ' AND r.from_entity_id IN (' || voted_subquery || ')';
  END IF;

  -- Value-less entities need a bounded candidate set. The TYPES relation gives
  -- one; so does the vote set, and the vote set is both cheaper and available
  -- without a type filter. Either will do.
  with_unscored := COALESCE(include_without_value, false)
    AND ((type_ids IS NOT NULL AND cardinality(type_ids) > 0) OR voted_by IS NOT NULL);

  IF NOT with_unscored THEN
    -- Backward-compatible path (identical to 0069 apart from the vote predicate):
    -- only entities WITH a usable value.
    sql :=
         'SELECT e.* FROM ('
      || '  SELECT DISTINCT ON (v.entity_id) v.entity_id, ' || sort_expr || ' AS sort_value'
      || '  FROM "values" v'
      || '  WHERE v.property_id = ' || quote_nullable(property_id) || ' AND ' || null_pred || space_pred || type_pred || voted_value_pred
      || '  ORDER BY v.entity_id, ' || sort_expr || ' ' || dir
      || ') sub JOIN entities e ON e.id = sub.entity_id'
      || ' ORDER BY sub.sort_value ' || dir || ', e.id';
  ELSE
    -- Null sorts as zero for numeric types; otherwise null sorts last in both directions.
    IF is_numeric THEN
      order_expr := 'COALESCE(sub.sort_value, 0) ' || dir;
    ELSE
      order_expr := 'sub.sort_value ' || dir || ' NULLS LAST';
    END IF;

    -- Candidates: entities of type_ids (within space_ids) when a type filter is
    -- given, otherwise the voted set on its own. When both are present the type
    -- filter still applies, so a Positions tab scoped to Claims stays scoped.
    IF type_ids IS NOT NULL AND cardinality(type_ids) > 0 THEN
      candidate_pred := format(
        'r.type_id = %L AND r.to_entity_id = ANY(%L::uuid[])',
        types_rel_id, type_ids) || rel_space_pred || voted_rel_pred;
      candidate_src :=
        '  SELECT DISTINCT r.from_entity_id AS entity_id FROM relations r WHERE ' || candidate_pred;
    ELSE
      candidate_src :=
        '  SELECT DISTINCT uv.object_id AS entity_id FROM ('
        || voted_subquery || ') uv(object_id)'
        || '  JOIN entities e2 ON e2.id = uv.object_id';
    END IF;

    sql :=
         'WITH scored AS ('
      || '  SELECT DISTINCT ON (v.entity_id) v.entity_id, ' || sort_expr || ' AS sort_value'
      || '  FROM "values" v'
      || '  WHERE v.property_id = ' || quote_nullable(property_id) || ' AND ' || null_pred || space_pred || type_pred || voted_value_pred
      || '  ORDER BY v.entity_id, ' || sort_expr || ' ' || dir
      || '), candidates AS ('
      || candidate_src
      || '), unscored AS ('
      || '  SELECT c.entity_id, NULL::' || null_cast || ' AS sort_value FROM candidates c'
      || '  WHERE NOT EXISTS (SELECT 1 FROM scored s WHERE s.entity_id = c.entity_id)'
      || ') SELECT e.* FROM ('
      || '  SELECT entity_id, sort_value FROM scored'
      || '  UNION ALL'
      || '  SELECT entity_id, sort_value FROM unscored'
      || ') sub JOIN entities e ON e.id = sub.entity_id'
      || ' ORDER BY ' || order_expr || ', e.id';
  END IF;

  RETURN QUERY EXECUTE sql;
END;
$fn$ LANGUAGE plpgsql STABLE;
