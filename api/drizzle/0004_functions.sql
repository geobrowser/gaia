CREATE OR REPLACE FUNCTION public.entities_name(entity entities) RETURNS text AS $$
  SELECT string FROM values WHERE entity_id = entity.id AND property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935' LIMIT 1;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION public.entities_description(entity entities) RETURNS text AS $$
  SELECT string FROM values WHERE entity_id = entity.id AND property_id = '9b1f76ff-9711-404c-861e-59dc3fa7d037' LIMIT 1;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION public.entities_types(entities entities) RETURNS SETOF public.entities AS $$
  SELECT e.*
  FROM entities e
  INNER JOIN relations r ON e.id = r.to_entity_id
  WHERE r.from_entity_id = entities.id AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1';
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION public.entities_type_ids(entities entities) RETURNS uuid[] AS $$
  SELECT array_agg(DISTINCT e.id)
  FROM entities e
  INNER JOIN relations r ON e.id = r.to_entity_id
  WHERE r.from_entity_id = entities.id AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1';
$$ LANGUAGE sql STABLE;

CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE INDEX IF NOT EXISTS values_text_gin_trgm_idx ON values USING GIN (string gin_trgm_ops);

-- Create a simplified fuzzy search function that only searches name and description properties
CREATE OR REPLACE FUNCTION public.search(
  query TEXT,
  space_id UUID DEFAULT NULL,
  similarity_threshold FLOAT DEFAULT 0.3
) RETURNS SETOF public.entities AS $$
  WITH search_values AS (
    SELECT
      v.entity_id,
      CASE
        WHEN v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935' THEN similarity(v.string, query) * 2.0  -- Name property
        WHEN v.property_id = '9b1f76ff-9711-404c-861e-59dc3fa7d037' THEN similarity(v.string, query) * 1.5  -- Description property
      END AS sim_score
    FROM
      values v
    WHERE
      v.string % query
      AND similarity(v.string, query) >= similarity_threshold
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

CREATE OR REPLACE FUNCTION public.entities_spaces_in(entity entities)
RETURNS SETOF public.spaces AS $$
  SELECT DISTINCT s.*
  FROM spaces s
  WHERE EXISTS (
    SELECT 1 FROM values v WHERE v.entity_id = entity.id AND v.space_id = s.id
    UNION ALL
    SELECT 1 FROM relations r WHERE r.from_entity_id = entity.id AND r.space_id = s.id
  )
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION public.entities_space_ids(entity entities)
RETURNS uuid[] AS $$
  SELECT COALESCE(
    ARRAY(
      SELECT DISTINCT space_id
      FROM (
        SELECT space_id FROM values WHERE entity_id = entity.id
        UNION ALL
        SELECT space_id FROM relations WHERE from_entity_id = entity.id
      ) AS all_spaces
      WHERE space_id IS NOT NULL
    ),
    '{}'::uuid[]
  );
$$ LANGUAGE sql STABLE;

/*
 * Returns the space front page entity for a given space
 * This finds the first entity that has a relation type of SystemIds.Types
 * and a toEntityId of SystemIds.SPACE_TYPE
 */
CREATE OR REPLACE FUNCTION public.spaces_page(space spaces)
RETURNS public.entities AS $$
  SELECT e.*
  FROM entities e
  JOIN relations r ON e.id = r.from_entity_id
  WHERE r.space_id = space.id
    AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1' -- SystemIds.Types
    AND r.to_entity_id = '362c1dbd-dc64-44bb-a3c4-652f38a642d7' -- SystemIds.SPACE_TYPE
  LIMIT 1;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION public.types(
  space_id UUID DEFAULT NULL
)
RETURNS SETOF public.entities AS $$
  SELECT e.*
  FROM entities e
  WHERE EXISTS (
    SELECT 1
    FROM relations r
    WHERE r.from_entity_id = e.id
      AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1' -- SystemIds.Types
      AND r.to_entity_id = 'e7d737c5-3676-4c60-9fa1-6aa64a8c90ad' -- SystemIds.TYPE
      AND (space_id IS NULL OR r.space_id = space_id)
  )
  ORDER BY e.id;
$$ LANGUAGE sql STABLE;

-- Simple function to get a single type entity by ID
CREATE OR REPLACE FUNCTION public.type(id UUID)
RETURNS public.entities AS $$
  SELECT e.*
  FROM entities e
  WHERE e.id = type.id
    AND EXISTS (
      SELECT 1
      FROM relations r
      WHERE r.from_entity_id = e.id
        AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1' -- SystemIds.Types
        AND r.to_entity_id = 'e7d737c5-3676-4c60-9fa1-6aa64a8c90ad' -- SystemIds.TYPE
    )
  LIMIT 1;
$$ LANGUAGE sql STABLE;

COMMENT ON FUNCTION public.types(UUID) IS E'@name typesList';

CREATE OR REPLACE FUNCTION public.entities_properties(
  entity entities,
  space_id UUID DEFAULT NULL
)
RETURNS SETOF public.properties AS $$
  WITH type_property_relations AS (
    -- Get all relations where this type has properties
    SELECT r.to_entity_id AS property_id
    FROM relations r
    WHERE r.from_entity_id = entity.id
      AND r.type_id = '01412f83-8189-4ab1-8365-65c7fd358cc1' -- SystemIds.PROPERTIES
      AND (space_id IS NULL OR r.space_id = space_id)
  ),
  -- Always include name and description properties
  system_properties AS (
    SELECT
      id,
      ROW_NUMBER() OVER (ORDER BY id) AS priority
    FROM properties
    WHERE id IN ('a126ca53-0c8e-48d5-b888-82c734c38935', '9b1f76ff-9711-404c-861e-59dc3fa7d037')
  ),
  -- Custom properties from relations
  custom_properties AS (
    SELECT
      p.id,
      100 + ROW_NUMBER() OVER (ORDER BY p.id) AS priority
    FROM properties p
    JOIN type_property_relations tpr ON p.id = tpr.property_id
    WHERE p.id NOT IN ('a126ca53-0c8e-48d5-b888-82c734c38935', '9b1f76ff-9711-404c-861e-59dc3fa7d037')
  ),
  -- Combine system and custom properties with priority ordering
  all_properties_with_priority AS (
    SELECT * FROM system_properties
    UNION ALL
    SELECT * FROM custom_properties
  )
  -- Get the final result with pagination
  SELECT p.*
  FROM properties p
  JOIN all_properties_with_priority app ON p.id = app.id
  ORDER BY app.priority
$$ LANGUAGE sql STABLE;

COMMENT ON FUNCTION public.entities_properties(entities, UUID) IS E'@fieldName properties\n@resultFieldName properties\n@resultListFieldName propertiesList';

/*
 * Returns the renderable type entity ID for a property
 */
CREATE OR REPLACE FUNCTION public.properties_renderable_type(properties properties)
RETURNS UUID AS $$
  -- Skip system properties (NAME and DESCRIPTION)
  -- SystemIds.NAME_PROPERTY = 'a126ca53-0c8e-48d5-b888-82c734c38935'
  -- SystemIds.DESCRIPTION_PROPERTY = '9b1f76ff-9711-404c-861e-59dc3fa7d037'
  SELECT CASE
    WHEN properties.id IN ('a126ca53-0c8e-48d5-b888-82c734c38935', '9b1f76ff-9711-404c-861e-59dc3fa7d037') THEN NULL
    ELSE (
      SELECT r.to_entity_id
      FROM relations r
      WHERE r.from_entity_id = properties.id
        AND r.type_id = '2316bbe1-c76f-4635-83f2-3e03b4f1fe46' -- RENDERABLE_TYPE_RELATION_ID
      LIMIT 1
    )
  END;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION public.properties_unit(properties properties)
RETURNS UUID AS $$
      SELECT r.to_entity_id
      FROM relations r
      WHERE r.from_entity_id = properties.id
        AND r.type_id = '11b06581-20d3-41ea-b570-2ef4ee0a4ffd' -- UNIT property id
      LIMIT 1
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION public.properties_format(properties properties)
RETURNS TEXT AS $$
      SELECT v.string
      FROM values v
      WHERE v.entity_id = properties.id
        AND v.property_id = '396f8c72-dfd0-4b57-91ea-09c1b9321b2f' -- format property id
      LIMIT 1
$$ LANGUAGE sql STABLE;

/*
 * Returns all relation value types for a property
 */
CREATE OR REPLACE FUNCTION public.properties_relation_value_types(properties properties)
RETURNS SETOF public.entities AS $$
  -- Skip system properties (NAME and DESCRIPTION)
  -- SystemIds.NAME_PROPERTY = 'a126ca53-0c8e-48d5-b888-82c734c38935'
  -- SystemIds.DESCRIPTION_PROPERTY = '9b1f76ff-9711-404c-861e-59dc3fa7d037'
  SELECT e.*
  FROM entities e
  JOIN relations r ON r.to_entity_id = e.id
  WHERE properties.id NOT IN ('a126ca53-0c8e-48d5-b888-82c734c38935', '9b1f76ff-9711-404c-861e-59dc3fa7d037')
    AND r.from_entity_id = properties.id
    AND r.type_id = '9eea393f-17dd-4971-a62e-a603e8bfec20' -- SystemIds.RELATION_VALUE_RELATIONSHIP_TYPE
  ORDER BY e.id
$$ LANGUAGE sql STABLE;

/*
 * Returns an array of relation value type IDs for a property
 */
CREATE OR REPLACE FUNCTION public.properties_relation_value_type_ids(properties properties)
RETURNS uuid[] AS $$
  -- Skip system properties (NAME and DESCRIPTION)
  -- SystemIds.NAME_PROPERTY = 'a126ca53-0c8e-48d5-b888-82c734c38935'
  -- SystemIds.DESCRIPTION_PROPERTY = '9b1f76ff-9711-404c-861e-59dc3fa7d037'
  SELECT array_agg(DISTINCT r.to_entity_id)
  FROM relations r
  WHERE properties.id NOT IN ('a126ca53-0c8e-48d5-b888-82c734c38935', '9b1f76ff-9711-404c-861e-59dc3fa7d037')
    AND r.from_entity_id = properties.id
    AND r.type_id = '9eea393f-17dd-4971-a62e-a603e8bfec20' -- SystemIds.RELATION_VALUE_RELATIONSHIP_TYPE
$$ LANGUAGE sql STABLE;

/**
* Returns name of a property
*/
CREATE OR REPLACE FUNCTION public.properties_name(property properties) RETURNS text AS $$
  SELECT string FROM values WHERE entity_id = property.id AND property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935' LIMIT 1;
$$ LANGUAGE sql STABLE;

/**
* Returns description of a property
*/
CREATE OR REPLACE FUNCTION public.properties_description(property properties) RETURNS text AS $$
  SELECT string FROM values WHERE entity_id = property.id AND property_id = '9b1f76ff-9711-404c-861e-59dc3fa7d037' LIMIT 1;
$$ LANGUAGE sql STABLE;

-- Rename properties.type to dataType in GraphQL schema
COMMENT ON COLUMN public.properties.type IS E'@name dataType';

COMMENT ON CONSTRAINT relations_type_id_properties_id_fk
ON relations
IS E'@fieldName type\n@omit many';

COMMENT ON CONSTRAINT relations_space_id_spaces_id_fk
ON relations
IS E'@fieldName space';

COMMENT ON CONSTRAINT relations_to_space_id_spaces_id_fk
ON relations
IS E'@fieldName toSpace';

COMMENT ON CONSTRAINT relations_from_space_id_spaces_id_fk
ON relations
IS E'@fieldName fromSpace';

COMMENT ON CONSTRAINT values_property_id_properties_id_fk
ON values
IS E'@fieldName property\n@omit many';

COMMENT ON CONSTRAINT values_space_id_spaces_id_fk
ON values
IS E'@fieldName space';

COMMENT ON CONSTRAINT values_entity_id_entities_id_fk
ON values
IS E'@fieldName entity\n@foreignFieldName values\n@foreignSimpleFieldName valuesList';

COMMENT ON CONSTRAINT relations_from_entity_id_entities_id_fk
ON relations
IS E'@fieldName fromEntity\n@foreignFieldName relations\n@foreignSimpleFieldName relationsList';

COMMENT ON CONSTRAINT relations_to_entity_id_entities_id_fk
ON relations
IS E'@fieldName toEntity\n@foreignFieldName backlinks\n@foreignSimpleFieldName backlinksList';

COMMENT ON CONSTRAINT relations_entity_id_entities_id_fk
ON relations
IS E'@fieldName entity\n@foreignFieldName relationsWhereEntity\n@foreignSimpleFieldName relationsWhereEntityList';

COMMENT ON CONSTRAINT members_space_id_spaces_id_fk
ON members
IS E'@fieldName space\n@foreignFieldName members\n@foreignSimpleFieldName membersList';

COMMENT ON CONSTRAINT editors_space_id_spaces_id_fk
ON editors
IS E'@fieldName space\n@foreignFieldName editors\n@foreignSimpleFieldName editorsList';

COMMENT ON CONSTRAINT members_address_space_id_pk ON MEMBERS IS E'@omit';
COMMENT ON CONSTRAINT editors_address_space_id_pk ON EDITORS IS E'@omit';

COMMENT ON TABLE ipfs_cache IS E'@omit';

-- Create enum for sort order if it doesn't exist
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'sort_order') THEN
        CREATE TYPE sort_order AS ENUM ('ASC', 'DESC');
    END IF;
END $$;

-- Custom query function to order entities by property value
CREATE OR REPLACE FUNCTION entities_ordered_by_property(
  property_id uuid,
  space_id uuid DEFAULT NULL,
  sort_direction sort_order DEFAULT 'ASC'
)
RETURNS SETOF entities AS $$
  WITH filtered_entities AS (
    SELECT DISTINCT
      e.id,
      e.created_at,
      e.created_at_block,
      e.updated_at,
      e.updated_at_block,
      p.type as property_type,
      v.string,
      v.number,
      v.boolean,
      v.time,
      v.point
    FROM entities e
    INNER JOIN values v ON v.entity_id = e.id
    INNER JOIN properties p ON v.property_id = p.id
    WHERE v.property_id = entities_ordered_by_property.property_id
      AND (entities_ordered_by_property.space_id IS NULL OR v.space_id = entities_ordered_by_property.space_id)
      AND (
        (p.type = 'String' AND v.string IS NOT NULL AND trim(v.string) != '') OR
        (p.type = 'Number' AND v.number IS NOT NULL) OR
        (p.type = 'Boolean' AND v.boolean IS NOT NULL) OR
        (p.type = 'Time' AND v.time IS NOT NULL AND trim(v.time) != '') OR
        (p.type = 'Point' AND v.point IS NOT NULL AND trim(v.point) != '') OR
        (p.type = 'Relation' AND FALSE)
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
        WHEN property_type = 'String' THEN string
        WHEN property_type = 'Boolean' THEN boolean::text
        WHEN property_type = 'Time' THEN time
        WHEN property_type = 'Point' THEN point
      END
    END ASC,
    CASE WHEN sort_direction = 'ASC' AND property_type = 'Number' THEN number::numeric END ASC,
    CASE WHEN sort_direction = 'DESC' THEN
      CASE
        WHEN property_type = 'String' THEN string
        WHEN property_type = 'Boolean' THEN boolean::text
        WHEN property_type = 'Time' THEN time
        WHEN property_type = 'Point' THEN point
      END
    END DESC,
    CASE WHEN sort_direction = 'DESC' AND property_type = 'Number' THEN number::numeric END DESC;
$$ LANGUAGE sql STABLE;
