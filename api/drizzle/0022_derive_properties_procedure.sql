-- Custom SQL migration file, put your code below! --

-- Properties are entities with TYPE relation pointing to the Property type.
-- SystemIds.Types = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'
-- SystemIds.PROPERTY_TYPE = '808a04ce-b21c-7d88-8ad1-2e240613e5ca'
--
-- TODO: Verify the following IDs before production:
--   1. PROPERTY_TYPE ID (808a04ce-b21c-7d88-8ad1-2e240613e5ca)
--   2. DATA_TYPE_PROPERTY ID (currently zero UUID placeholder)
--   3. Data type entity IDs (if data types are stored as relations to entities)

-- Function to return all property entities (entities typed as Property)
CREATE OR REPLACE FUNCTION public.properties(
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
      AND r.to_entity_id = '808a04ce-b21c-7d88-8ad1-2e240613e5ca' -- SystemIds.PROPERTY_TYPE
      AND (space_id IS NULL OR r.space_id = space_id)
  )
  ORDER BY e.id;
$$ LANGUAGE sql STABLE;

COMMENT ON FUNCTION public.properties(UUID) IS E'@name propertiesList';

-- Function to get a single property entity by ID
CREATE OR REPLACE FUNCTION public.property(id UUID)
RETURNS public.entities AS $$
  SELECT e.*
  FROM entities e
  WHERE e.id = property.id
    AND EXISTS (
      SELECT 1
      FROM relations r
      WHERE r.from_entity_id = e.id
        AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1' -- SystemIds.Types
        AND r.to_entity_id = '808a04ce-b21c-7d88-8ad1-2e240613e5ca' -- SystemIds.PROPERTY_TYPE
    )
  LIMIT 1;
$$ LANGUAGE sql STABLE;

-- Computed field: returns the data type for a property entity
-- The data type is stored as a value on the property entity
CREATE OR REPLACE FUNCTION public.entities_data_type(entity entities)
RETURNS TEXT AS $$
  SELECT v.string
  FROM values v
  WHERE v.entity_id = entity.id
    AND v.property_id = '00000000-0000-0000-0000-000000000000' -- TODO: Replace with actual DATA_TYPE_PROPERTY ID
  LIMIT 1;
$$ LANGUAGE sql STABLE;
