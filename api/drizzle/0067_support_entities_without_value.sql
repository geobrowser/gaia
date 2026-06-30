-- Support returning entities WITHOUT a value for the ordered property in
-- entities_ordered_by_property (and its PostGraphile *Connection field).
--
-- Background: the function (see 0060) drives entirely from the "values" table and applies
-- a per-type NOT NULL / non-empty predicate, so only entities that HAVE a usable value for
-- `property_id` can ever appear. Entities that match the space/type filters but have no
-- value for the property were never even candidates (this is the reported bug: a scored
-- "table view" only returned the handful of entities that happened to have a score).
--
-- This adds an opt-in `include_without_value` parameter (default false, so existing queries
-- are byte-for-byte unchanged — same SQL, same plan). When true, entities of `type_ids`
-- within `space_ids` that have no usable value for the property are unioned in with a NULL
-- sort value.
--
-- Ordering of the value-less entities (per product guidance: "scores can be negative, so
-- null should be considered the same as zero"):
--   * numeric data types (integer/float/decimal): the NULL sort value is ordered as 0 via
--     COALESCE(sort_value, 0). DESC => positives -> zero/null -> negatives; ASC the reverse.
--     Entities with a literal 0 and value-less entities sort together, which is intended.
--   * non-numeric types (text/date/time/datetime/boolean/point): "zero" is undefined, so
--     value-less entities are ordered last (NULLS LAST) in both ASC and DESC.
--
-- The value-less candidate set is anchored on the TYPES relation
-- (8f151ba4-de20-4e3c-9cb4-99ddf96f48f1) because entities carry no space/type column of
-- their own. `include_without_value` therefore requires `type_ids`; when it is true but no
-- type_ids are supplied the function falls back to scored-only (backward-compatible) output.
--
-- Note: an entity whose only value fails the null/non-empty predicate (e.g. empty text, or
-- a row with a NULL typed column) is treated as value-less and surfaces via the unscored
-- branch, which matches the "no usable value" intent.

DROP FUNCTION IF EXISTS public.entities_ordered_by_property(uuid, uuid[], sort_order, text);
--> statement-breakpoint
DROP FUNCTION IF EXISTS public.entities_ordered_by_property(uuid, uuid[], sort_order, text, uuid);
--> statement-breakpoint
DROP FUNCTION IF EXISTS public.entities_ordered_by_property(uuid, uuid[], sort_order, text, uuid[]);
--> statement-breakpoint
CREATE OR REPLACE FUNCTION public.entities_ordered_by_property(
  property_id uuid,
  space_ids uuid[] DEFAULT NULL,
  sort_direction sort_order DEFAULT 'ASC',
  data_type text DEFAULT NULL,
  type_ids uuid[] DEFAULT NULL,
  include_without_value boolean DEFAULT false
)
RETURNS SETOF entities AS $fn$
DECLARE
  resolved_type   text;
  sort_expr       text;
  null_pred       text;
  null_cast       text;
  is_numeric      boolean := false;
  dir             text;
  space_pred      text := '';
  type_pred       text := '';
  rel_space_pred  text := '';
  candidate_pred  text;
  order_expr      text;
  with_unscored   boolean;
  sql             text;
BEGIN
  -- include_without_value requires space_ids. The value-less branch enumerates the full
  -- type population and the whole set is ordered/materialized before PostGraphile applies
  -- LIMIT/OFFSET (pagination happens outside this function, see 0060), so an unbounded
  -- (all-spaces) candidate scan could be very large. Requiring space_ids bounds the scan to
  -- the requested spaces. COALESCE so an explicit NULL flag behaves like false (no guard).
  IF COALESCE(include_without_value, false) AND (space_ids IS NULL OR cardinality(space_ids) = 0) THEN
    RAISE EXCEPTION 'space_ids is required when include_without_value is true'
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
      '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1', type_ids);
  END IF;

  -- Value-less entities can only be bounded by the TYPES relation; without type_ids there is
  -- no cheap, well-defined candidate set, so fall back to scored-only behavior. COALESCE guards
  -- the nullable flag: PostGraphile exposes includeWithoutValue as a nullable Boolean, and a
  -- NULL must behave like false (NULL AND ... yields NULL, which would otherwise fall through
  -- the IF NOT below into the unscored branch).
  with_unscored := COALESCE(include_without_value, false) AND type_ids IS NOT NULL AND cardinality(type_ids) > 0;

  IF NOT with_unscored THEN
    -- Backward-compatible path (identical to 0060): only entities WITH a usable value.
    sql :=
         'SELECT e.* FROM ('
      || '  SELECT DISTINCT ON (v.entity_id) v.entity_id, ' || sort_expr || ' AS sort_value'
      || '  FROM "values" v'
      || '  WHERE v.property_id = ' || quote_nullable(property_id) || ' AND ' || null_pred || space_pred || type_pred
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

    -- Candidate set for value-less entities: entities of type_ids (within space_ids).
    -- '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1' is TYPES_RELATION_ID, the system "Types" relation
    -- that links an entity (from_entity_id) to its type entity (to_entity_id).
    candidate_pred := format(
      'r.type_id = %L AND r.to_entity_id = ANY(%L::uuid[])',
      '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1', type_ids) || rel_space_pred;

    sql :=
         'WITH scored AS ('
      || '  SELECT DISTINCT ON (v.entity_id) v.entity_id, ' || sort_expr || ' AS sort_value'
      || '  FROM "values" v'
      || '  WHERE v.property_id = ' || quote_nullable(property_id) || ' AND ' || null_pred || space_pred || type_pred
      || '  ORDER BY v.entity_id, ' || sort_expr || ' ' || dir
      || '), candidates AS ('
      || '  SELECT DISTINCT r.from_entity_id AS entity_id FROM relations r WHERE ' || candidate_pred
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
