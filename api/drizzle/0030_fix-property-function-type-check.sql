-- Fix property(id) function to verify entity is typed as Property
--
-- The previous implementation only checked if an entity exists with the given ID,
-- but didn't verify that the entity has a Types relation to PROPERTY_TYPE.
-- This caused the API to return property_info for any entity, not just properties.

CREATE OR REPLACE FUNCTION public.property(id uuid)
RETURNS property_info AS $$
  SELECT build_property_info(property.id)
  WHERE EXISTS (
    SELECT 1 
    FROM relations r 
    WHERE r.from_entity_id = property.id
      AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1' -- TYPES_RELATION_ID
      AND r.to_entity_id = '808a04ce-b21c-4d88-8ad1-2e240613e5ca' -- PROPERTY_TYPE
  );
$$ LANGUAGE sql STABLE;
