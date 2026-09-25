-- entitiesOrderedByPropertyConnection: add entity_ids.
--
-- geogenesis data blocks and collections restrict this connection to a list of
-- entities with `filter: { id: { in: [...] } }`. That filter applies OUTSIDE the
-- function, after it has built and sorted its whole result (pagination and
-- filters wrap the function, see 0060 and 0088). With no space or type bounds
-- the result is every entity that has the property, so sorting one entity by
-- name sorted every named entity first.
--
-- Measured on testnet 2026-09-25, the same answer (1 row):
--   today, outer filter over the function      11,960 ms
--   entity_ids inside the function              0.2 ms (index scan on
--                                               values_name_entity_idx)
-- The API logs showed these calls at 12-25 s from geobrowser.io.
--
-- This is the same move 0088 made for voted_by: push the restriction inside so
-- it bounds the scan. entity_ids applies to the scored values scan and to the
-- value-less candidate set, and like voted_by it is enough on its own to allow
-- include_without_value, because the ids ARE a bounded candidate set.
--
-- An empty array means "no restriction", as space_ids and type_ids do. Callers
-- must not send an empty list meaning "match nothing".
--
-- The new parameter is LAST and defaulted: the migration runs before api pods
-- restart, and pods still holding the old schema call the function without it.
--
-- Drop the 0088 signature first: a differing parameter list is an overload, not
-- a replacement, and two overloads leave postgraphile to pick one.
DROP FUNCTION IF EXISTS public.entities_ordered_by_property(uuid, uuid[], sort_order, text, uuid[], boolean, uuid, smallint[]);
--> statement-breakpoint

CREATE OR REPLACE FUNCTION public.entities_ordered_by_property(
  property_id uuid,
  space_ids uuid[] DEFAULT NULL,
  sort_direction sort_order DEFAULT 'ASC',
  data_type text DEFAULT NULL,
  type_ids uuid[] DEFAULT NULL,
  include_without_value boolean DEFAULT false,
  voted_by uuid DEFAULT NULL,
  voted_by_kinds smallint[] DEFAULT NULL,
  entity_ids uuid[] DEFAULT NULL
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
  ids_value_pred  text := '';
  ids_rel_pred    text := '';
  has_ids         boolean := entity_ids IS NOT NULL AND cardinality(entity_ids) > 0;
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
     AND voted_by IS NULL
     AND NOT has_ids THEN
    RAISE EXCEPTION 'space_ids, voted_by or entity_ids is required when include_without_value is true'
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

  -- entity_ids bounds the scan the same way voted_by does, and more tightly:
  -- values_entity_property_idx (entity_id, property_id) answers it directly.
  IF has_ids THEN
    ids_value_pred := format(' AND v.entity_id = ANY(%L::uuid[])', entity_ids);
    ids_rel_pred   := format(' AND r.from_entity_id = ANY(%L::uuid[])', entity_ids);
  END IF;

  -- Value-less entities need a bounded candidate set. The TYPES relation gives
  -- one; so does the vote set, and the vote set is both cheaper and available
  -- without a type filter. Either will do.
  with_unscored := COALESCE(include_without_value, false)
    AND ((type_ids IS NOT NULL AND cardinality(type_ids) > 0) OR voted_by IS NOT NULL OR has_ids);

  IF NOT with_unscored THEN
    -- Backward-compatible path (identical to 0069 apart from the vote predicate):
    -- only entities WITH a usable value.
    sql :=
         'SELECT e.* FROM ('
      || '  SELECT DISTINCT ON (v.entity_id) v.entity_id, ' || sort_expr || ' AS sort_value'
      || '  FROM "values" v'
      || '  WHERE v.property_id = ' || quote_nullable(property_id) || ' AND ' || null_pred || space_pred || type_pred || voted_value_pred || ids_value_pred
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
        types_rel_id, type_ids) || rel_space_pred || voted_rel_pred || ids_rel_pred;
      candidate_src :=
        '  SELECT DISTINCT r.from_entity_id AS entity_id FROM relations r WHERE ' || candidate_pred;
    ELSIF voted_by IS NOT NULL THEN
      candidate_src :=
        '  SELECT DISTINCT uv.object_id AS entity_id FROM ('
        || voted_subquery || ') uv(object_id)'
        || '  JOIN entities e2 ON e2.id = uv.object_id';
      IF has_ids THEN
        candidate_src := candidate_src
          || format(' WHERE uv.object_id = ANY(%L::uuid[])', entity_ids);
      END IF;
    ELSE
      -- Only entity_ids bounds the candidates: they are the candidates.
      candidate_src := format(
        '  SELECT e2.id AS entity_id FROM entities e2 WHERE e2.id = ANY(%L::uuid[])',
        entity_ids);
    END IF;

    sql :=
         'WITH scored AS ('
      || '  SELECT DISTINCT ON (v.entity_id) v.entity_id, ' || sort_expr || ' AS sort_value'
      || '  FROM "values" v'
      || '  WHERE v.property_id = ' || quote_nullable(property_id) || ' AND ' || null_pred || space_pred || type_pred || voted_value_pred || ids_value_pred
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
