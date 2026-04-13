-- Fix entities_ordered_by_property to include entities missing the sorted property.
-- Previously used INNER JOIN on values, which excluded entities without a value for the
-- sorted property. Now uses LEFT JOIN with NULLS LAST so those entities appear at the end.

CREATE OR REPLACE FUNCTION entities_ordered_by_property(
  property_id uuid,
  space_id uuid DEFAULT NULL,
  sort_direction sort_order DEFAULT 'ASC',
  data_type text DEFAULT NULL  -- 'text', 'integer', 'float', 'decimal', 'boolean', 'date', 'time', 'datetime', 'point'
)
RETURNS SETOF entities AS $$
  WITH resolved_type AS (
    -- Use provided data_type, or look up via DATA_TYPE_ID relation
    SELECT COALESCE(
      data_type,
      (SELECT v."text" FROM "values" v
       JOIN relations r ON r.to_entity_id = v.entity_id
       WHERE r.from_entity_id = entities_ordered_by_property.property_id
       AND r.type_id = '6d29d578-49bb-4959-baf7-2cc696b1671a' -- DATA_TYPE_PROPERTY_ID
       AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935' -- NAME_PROPERTY_ID
       LIMIT 1)
    ) AS property_type
  ),
  space_entities AS (
    -- Get all distinct entities that exist in the given space (or all entities if no space filter)
    SELECT DISTINCT e.id, e.created_at, e.created_at_block, e.updated_at, e.updated_at_block
    FROM entities e
    JOIN "values" sv ON sv.entity_id = e.id
    WHERE (entities_ordered_by_property.space_id IS NULL OR sv.space_id = entities_ordered_by_property.space_id)
  ),
  ordered_entities AS (
    SELECT DISTINCT
      se.id,
      se.created_at,
      se.created_at_block,
      se.updated_at,
      se.updated_at_block,
      rt.property_type,
      NULLIF(trim(v."text"), '') AS text_val,
      v.integer AS integer_val,
      v.float AS float_val,
      v."decimal" AS decimal_val,
      v.boolean AS boolean_val,
      NULLIF(trim(v.date), '') AS date_val,
      NULLIF(trim(v."time"), '') AS time_val,
      NULLIF(trim(v.datetime), '') AS datetime_val,
      NULLIF(trim(v.point), '') AS point_val
    FROM space_entities se
    LEFT JOIN "values" v ON v.entity_id = se.id
      AND v.property_id = entities_ordered_by_property.property_id
      AND (entities_ordered_by_property.space_id IS NULL OR v.space_id = entities_ordered_by_property.space_id)
    CROSS JOIN resolved_type rt
  )
  SELECT
    id,
    created_at,
    created_at_block,
    updated_at,
    updated_at_block
  FROM ordered_entities
  ORDER BY
    CASE WHEN sort_direction = 'ASC' THEN
      CASE
        WHEN property_type = 'text' THEN text_val
        WHEN property_type = 'boolean' THEN boolean_val::text
        WHEN property_type = 'date' THEN date_val
        WHEN property_type = 'time' THEN time_val
        WHEN property_type = 'datetime' THEN datetime_val
        WHEN property_type = 'point' THEN point_val
      END
    END ASC NULLS LAST,
    CASE WHEN sort_direction = 'ASC' AND property_type = 'integer' THEN integer_val END ASC NULLS LAST,
    CASE WHEN sort_direction = 'ASC' AND property_type = 'float' THEN float_val END ASC NULLS LAST,
    CASE WHEN sort_direction = 'ASC' AND property_type = 'decimal' THEN decimal_val::numeric END ASC NULLS LAST,
    CASE WHEN sort_direction = 'DESC' THEN
      CASE
        WHEN property_type = 'text' THEN text_val
        WHEN property_type = 'boolean' THEN boolean_val::text
        WHEN property_type = 'date' THEN date_val
        WHEN property_type = 'time' THEN time_val
        WHEN property_type = 'datetime' THEN datetime_val
        WHEN property_type = 'point' THEN point_val
      END
    END DESC NULLS LAST,
    CASE WHEN sort_direction = 'DESC' AND property_type = 'integer' THEN integer_val END DESC NULLS LAST,
    CASE WHEN sort_direction = 'DESC' AND property_type = 'float' THEN float_val END DESC NULLS LAST,
    CASE WHEN sort_direction = 'DESC' AND property_type = 'decimal' THEN decimal_val::numeric END DESC NULLS LAST;
$$ LANGUAGE sql STABLE;
