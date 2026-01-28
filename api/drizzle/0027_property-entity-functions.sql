-- Custom SQL migration file for property entity functions
-- Properties are entities, but we expose them as a custom type with all metadata resolved

-- Drop existing functions that conflict with new return types
DROP FUNCTION IF EXISTS public.property(uuid);
DROP FUNCTION IF EXISTS public.properties(uuid);

-- Drop the old entities_data_type function - we don't want dataType on all entities
DROP FUNCTION IF EXISTS public.entities_data_type(entities);

-- Drop obsolete functions that reference the old 'properties' table (from 0004_functions.sql)
DROP FUNCTION IF EXISTS public.entities_properties(entities, uuid);
DROP FUNCTION IF EXISTS public.properties_renderable_type(properties);
DROP FUNCTION IF EXISTS public.properties_unit(properties);
DROP FUNCTION IF EXISTS public.properties_format(properties);
DROP FUNCTION IF EXISTS public.properties_relation_value_types(properties);
DROP FUNCTION IF EXISTS public.properties_relation_value_type_ids(properties);
DROP FUNCTION IF EXISTS public.properties_name(properties);
DROP FUNCTION IF EXISTS public.properties_description(properties);
DROP FUNCTION IF EXISTS public.entities_ordered_by_property(uuid, uuid, sort_order);

-- Fix functions that reference old 'string' column (now 'text')
CREATE OR REPLACE FUNCTION public.entities_name(entity entities) RETURNS text AS $$
  SELECT "text" FROM "values" WHERE entity_id = entity.id AND property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935' LIMIT 1;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION public.entities_description(entity entities) RETURNS text AS $$
  SELECT "text" FROM "values" WHERE entity_id = entity.id AND property_id = '9b1f76ff-9711-404c-861e-59dc3fa7d037' LIMIT 1;
$$ LANGUAGE sql STABLE;

-- Recreate index on 'text' column instead of 'string'
DROP INDEX IF EXISTS values_text_gin_trgm_idx;
CREATE INDEX IF NOT EXISTS values_text_gin_trgm_idx ON "values" USING GIN ("text" gin_trgm_ops);

-- Fix search function to use 'text' instead of 'string'
CREATE OR REPLACE FUNCTION public.search(
  query TEXT,
  space_id UUID DEFAULT NULL,
  similarity_threshold FLOAT DEFAULT 0.3
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
      v."text" % query
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
    ORDER BY
      max_score DESC, sv.entity_id
  )
  SELECT e.*
  FROM
    ranked_entities re
    JOIN entities e ON e.id = re.entity_id
  ORDER BY
    re.max_score DESC, e.id;
$$ LANGUAGE sql STABLE;

-- Update table comments to rename FK fields so they don't conflict with our computed functions
-- We rename property -> propertyEntity and type -> typeEntity for the FK relations
COMMENT ON TABLE "values" IS E'@foreignKey (property_id) references entities (id)|@fieldName propertyEntity|@omit many\n@foreignKey (entity_id) references entities (id)|@fieldName entity|@foreignFieldName values|@foreignSimpleFieldName valuesList\n@foreignKey (space_id) references spaces (id)|@fieldName space';

COMMENT ON TABLE "relations" IS E'@foreignKey (type_id) references entities (id)|@fieldName typeEntity|@omit many\n@foreignKey (entity_id) references entities (id)|@fieldName entity|@foreignFieldName relationsWhereEntity|@foreignSimpleFieldName relationsWhereEntityList\n@foreignKey (from_entity_id) references entities (id)|@fieldName fromEntity|@foreignFieldName relations|@foreignSimpleFieldName relationsList\n@foreignKey (to_entity_id) references entities (id)|@fieldName toEntity|@foreignFieldName backlinks|@foreignSimpleFieldName backlinksList\n@foreignKey (space_id) references spaces (id)|@fieldName space\n@foreignKey (from_space_id) references spaces (id)|@fieldName fromSpace\n@foreignKey (to_space_id) references spaces (id)|@fieldName toSpace';

-- System IDs
-- NAME_PROPERTY_ID = 'a126ca53-0c8e-48d5-b888-82c734c38935'
-- DESCRIPTION_PROPERTY_ID = '9b1f76ff-9711-404c-861e-59dc3fa7d037'
-- TYPES_RELATION_ID = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'
-- PROPERTY_TYPE = '808a04ce-b21c-4d88-8ad1-2e240613e5ca'
-- DATA_TYPE_PROPERTY_ID = '6d29d578-49bb-4959-baf7-2cc696b1671a'
-- RENDERABLE_TYPE_PROPERTY_ID = '2316bbe1-c76f-4635-83f2-3e03b4f1fe46'
--
-- Native Data Type Entity IDs:
-- Boolean = '7aa4792e-eacd-4186-8272-fa7fc18298ac'
-- Integer = '149fd752-d9d0-4f80-820d-1d942eea7841'
-- Float64 = '9b597aae-c31c-46c8-8565-a370da0c2a65'
-- Decimal = 'a3288c22-a056-4f6f-b409-fbcccb2c118c'
-- Text = '9edb6fcc-e454-4aa5-8611-39d7f024c010'
-- Bytes = '66b43324-7667-4968-99b4-8a89bd1de22b'
-- Date = 'e661d102-9279-4449-a223-67dbae1be05a'
-- Time = 'ad75102b-03c0-4d59-9038-13ede9482742'
-- Datetime = '167664f6-68f8-40e1-976b-20bd16ed8d47'
-- Schedule = 'caf4dd12-ba48-44b9-9171-aff6c1313b50'
-- Point = 'df250d17-e364-413d-9779-2ddaae841e34'
-- Embedding = 'f7328493-78ba-4577-a33f-ac5f1c964f18'
-- Relation = '4b6d9fc1-fbfe-474c-861c-83398e1b50d9'

-- Create the property_info composite type
CREATE TYPE property_info AS (
  id uuid,
  name text,
  description text,
  data_type_id uuid,
  data_type_name text,
  renderable_type_id uuid,
  renderable_type_name text
);

/**
 * Helper function to build property_info from an entity ID
 */
CREATE OR REPLACE FUNCTION public.build_property_info(entity_id uuid)
RETURNS property_info AS $$
  SELECT ROW(
    entity_id,
    -- name
    (SELECT "text" FROM "values" WHERE entity_id = build_property_info.entity_id
     AND property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935' LIMIT 1),
    -- description
    (SELECT "text" FROM "values" WHERE entity_id = build_property_info.entity_id
     AND property_id = '9b1f76ff-9711-404c-861e-59dc3fa7d037' LIMIT 1),
    -- data_type_id (via DATA_TYPE relation)
    (SELECT r.to_entity_id FROM relations r
     WHERE r.from_entity_id = build_property_info.entity_id
     AND r.type_id = '6d29d578-49bb-4959-baf7-2cc696b1671a' LIMIT 1),
    -- data_type_name
    (SELECT v."text" FROM "values" v
     JOIN relations r ON r.to_entity_id = v.entity_id
     WHERE r.from_entity_id = build_property_info.entity_id
     AND r.type_id = '6d29d578-49bb-4959-baf7-2cc696b1671a'
     AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935' LIMIT 1),
    -- renderable_type_id (via RENDERABLE_TYPE relation)
    (SELECT r.to_entity_id FROM relations r
     WHERE r.from_entity_id = build_property_info.entity_id
     AND r.type_id = '2316bbe1-c76f-4635-83f2-3e03b4f1fe46' LIMIT 1),
    -- renderable_type_name
    (SELECT v."text" FROM "values" v
     JOIN relations r ON r.to_entity_id = v.entity_id
     WHERE r.from_entity_id = build_property_info.entity_id
     AND r.type_id = '2316bbe1-c76f-4635-83f2-3e03b4f1fe46'
     AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935' LIMIT 1)
  )::property_info;
$$ LANGUAGE sql STABLE;

/**
 * Computed field on values to resolve the property as property_info
 */
CREATE OR REPLACE FUNCTION public.values_property(v "values")
RETURNS property_info AS $$
  SELECT build_property_info(v.property_id);
$$ LANGUAGE sql STABLE;

COMMENT ON FUNCTION public.values_property("values") IS E'@fieldName property';

/**
 * Computed field on relations to resolve the type as property_info
 */
CREATE OR REPLACE FUNCTION public.relations_type(r relations)
RETURNS property_info AS $$
  SELECT build_property_info(r.type_id);
$$ LANGUAGE sql STABLE;

COMMENT ON FUNCTION public.relations_type(relations) IS E'@fieldName type';

/**
 * Root query: Get a single property by ID
 */
CREATE OR REPLACE FUNCTION public.property(id uuid)
RETURNS property_info AS $$
  SELECT build_property_info(property.id)
  WHERE EXISTS (
    SELECT 1 FROM entities e WHERE e.id = property.id
  );
$$ LANGUAGE sql STABLE;

/**
 * Root query: List all properties
 */
CREATE OR REPLACE FUNCTION public.properties(
  space_id uuid DEFAULT NULL
)
RETURNS SETOF property_info AS $$
  SELECT build_property_info(e.id)
  FROM entities e
  WHERE EXISTS (
    SELECT 1
    FROM relations r
    WHERE r.from_entity_id = e.id
      AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1' -- TYPES_RELATION_ID
      AND r.to_entity_id = '808a04ce-b21c-4d88-8ad1-2e240613e5ca' -- PROPERTY_TYPE
      AND (space_id IS NULL OR r.space_id = space_id)
  )
  ORDER BY e.id;
$$ LANGUAGE sql STABLE;


/**
 * Order entities by a property value.
 * If data_type is provided, use that. Otherwise look up the canonical type via DATA_TYPE_ID relation.
 *
 * GRC-20 v2 data types: boolean, integer, float, decimal, text, bytes, date, time, datetime, schedule, point, embedding
 */
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
  filtered_entities AS (
    SELECT DISTINCT
      e.id,
      e.created_at,
      e.created_at_block,
      e.updated_at,
      e.updated_at_block,
      rt.property_type,
      v."text" AS text_val,
      v.integer AS integer_val,
      v.float AS float_val,
      v."decimal" AS decimal_val,
      v.boolean AS boolean_val,
      v.date AS date_val,
      v."time" AS time_val,
      v.datetime AS datetime_val,
      v.point AS point_val
    FROM entities e
    INNER JOIN "values" v ON v.entity_id = e.id
    CROSS JOIN resolved_type rt
    WHERE v.property_id = entities_ordered_by_property.property_id
      AND (entities_ordered_by_property.space_id IS NULL OR v.space_id = entities_ordered_by_property.space_id)
      AND (
        (rt.property_type = 'text' AND v."text" IS NOT NULL AND trim(v."text") != '') OR
        (rt.property_type = 'integer' AND v.integer IS NOT NULL) OR
        (rt.property_type = 'float' AND v.float IS NOT NULL) OR
        (rt.property_type = 'decimal' AND v."decimal" IS NOT NULL) OR
        (rt.property_type = 'boolean' AND v.boolean IS NOT NULL) OR
        (rt.property_type = 'date' AND v.date IS NOT NULL AND trim(v.date) != '') OR
        (rt.property_type = 'time' AND v."time" IS NOT NULL AND trim(v."time") != '') OR
        (rt.property_type = 'datetime' AND v.datetime IS NOT NULL AND trim(v.datetime) != '') OR
        (rt.property_type = 'point' AND v.point IS NOT NULL AND trim(v.point) != '')
      )
  )
  SELECT
    id,
    created_at,
    created_at_block,
    updated_at,
    updated_at_block
  FROM filtered_entities
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
    END ASC,
    CASE WHEN sort_direction = 'ASC' AND property_type = 'integer' THEN integer_val END ASC,
    CASE WHEN sort_direction = 'ASC' AND property_type = 'float' THEN float_val END ASC,
    CASE WHEN sort_direction = 'ASC' AND property_type = 'decimal' THEN decimal_val::numeric END ASC,
    CASE WHEN sort_direction = 'DESC' THEN
      CASE
        WHEN property_type = 'text' THEN text_val
        WHEN property_type = 'boolean' THEN boolean_val::text
        WHEN property_type = 'date' THEN date_val
        WHEN property_type = 'time' THEN time_val
        WHEN property_type = 'datetime' THEN datetime_val
        WHEN property_type = 'point' THEN point_val
      END
    END DESC,
    CASE WHEN sort_direction = 'DESC' AND property_type = 'integer' THEN integer_val END DESC,
    CASE WHEN sort_direction = 'DESC' AND property_type = 'float' THEN float_val END DESC,
    CASE WHEN sort_direction = 'DESC' AND property_type = 'decimal' THEN decimal_val::numeric END DESC;
$$ LANGUAGE sql STABLE;
