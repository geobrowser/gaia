-- GEO-675: optimize entities_ordered_by_property (sorted table/collection views).
--
-- Orders entities by one of a property's typed values, filter-first: it drives from the
-- selective space/type set via existing relations/values indexes and a type semi-join,
-- instead of the old version which sorted the whole property set with a non-indexable
-- ORDER BY CASE before filtering. The sort column is resolved from `data_type`, or the
-- property's DATA_TYPE relation (ORDER BY r.id for determinism; empty set on an
-- unsupported/unresolved type). Predicates are appended only for the args present (no
-- "arg IS NULL OR ..." guards) so the planner can use indexes and the type semi-join.
-- `type_ids` is an optional, space-scoped NECESSARY type superset (OR semantics) — the app
-- still applies the exact filter as a PostGraphile residual, and omitting it reproduces the
-- prior no-type behavior (backward compatible). DISTINCT ON returns one row per entity; the
-- e.id tie-break keeps pagination stable.
--
-- No dedicated sort indexes
-- Type-filtered sorts (the common path) are already index-driven via the existing
-- relations/values indexes; space-only sorts fall back to a bounded sort.

DROP FUNCTION IF EXISTS public.entities_ordered_by_property(uuid, uuid, sort_order, text);
--> statement-breakpoint
DROP FUNCTION IF EXISTS public.entities_ordered_by_property(uuid, uuid, sort_order, text, uuid);
--> statement-breakpoint
CREATE OR REPLACE FUNCTION public.entities_ordered_by_property(
  property_id uuid,
  space_id uuid DEFAULT NULL,
  sort_direction sort_order DEFAULT 'ASC',
  data_type text DEFAULT NULL,
  type_ids uuid[] DEFAULT NULL
)
RETURNS SETOF entities AS $fn$
DECLARE
  resolved_type text;
  sort_expr     text;
  null_pred     text;
  dir           text;
  space_pred    text := '';
  type_pred     text := '';
  sql           text;
BEGIN
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

  CASE resolved_type
    WHEN 'text'     THEN sort_expr := 'left(v."text", 1024)'; null_pred := 'v."text" IS NOT NULL AND length(btrim(v."text")) > 0';
    WHEN 'integer'  THEN sort_expr := 'v.integer';            null_pred := 'v.integer IS NOT NULL';
    WHEN 'float'    THEN sort_expr := 'v.float';              null_pred := 'v.float IS NOT NULL';
    WHEN 'decimal'  THEN sort_expr := 'v."decimal"';          null_pred := 'v."decimal" IS NOT NULL';
    WHEN 'boolean'  THEN sort_expr := 'v.boolean';            null_pred := 'v.boolean IS NOT NULL';
    WHEN 'date'     THEN sort_expr := 'v.date';               null_pred := 'v.date IS NOT NULL AND length(btrim(v.date)) > 0';
    WHEN 'time'     THEN sort_expr := 'v."time"';             null_pred := 'v."time" IS NOT NULL AND length(btrim(v."time")) > 0';
    WHEN 'datetime' THEN sort_expr := 'v.datetime';           null_pred := 'v.datetime IS NOT NULL AND length(btrim(v.datetime)) > 0';
    WHEN 'point'    THEN sort_expr := 'v.point';              null_pred := 'v.point IS NOT NULL AND length(btrim(v.point)) > 0';
    ELSE
      RETURN;
  END CASE;

  dir := CASE WHEN sort_direction::text = 'DESC' THEN 'DESC' ELSE 'ASC' END;

  IF space_id IS NOT NULL THEN
    space_pred := format(' AND v.space_id = %L', space_id);
  END IF;

  IF type_ids IS NOT NULL AND cardinality(type_ids) > 0 THEN
    type_pred := format(
      ' AND EXISTS (SELECT 1 FROM relations r WHERE r.from_entity_id = v.entity_id AND r.type_id = %L AND r.to_entity_id = ANY(%L::uuid[]) AND r.space_id = v.space_id)',
      '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1', type_ids);
  END IF;

  -- OPTIMIZE (follow-up): LIMIT/OFFSET are applied by PostGraphile outside this function, so
  -- PG materializes the full ordered set before paginating. Pushing them inside would be
  -- limit-early but breaks cursor pagination; deferred since the type-prefiltered set is small.
  sql :=
       'SELECT e.* FROM ('
    || '  SELECT DISTINCT ON (v.entity_id) v.entity_id, ' || sort_expr || ' AS sort_value'
    || '  FROM "values" v'
    || '  WHERE v.property_id = ' || quote_nullable(property_id) || ' AND ' || null_pred || space_pred || type_pred
    || '  ORDER BY v.entity_id, ' || sort_expr || ' ' || dir
    || ') sub JOIN entities e ON e.id = sub.entity_id'
    || ' ORDER BY sub.sort_value ' || dir || ', e.id';

  RETURN QUERY EXECUTE sql;
END;
$fn$ LANGUAGE plpgsql STABLE;
