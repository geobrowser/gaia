-- Custom SQL migration file, put your code below! --

-- Include System Type relations in entities.typeIds
--
-- entities_type_ids() (originally defined in 0004_functions.sql) previously
-- aggregated only regular Type relations (TYPES_PROPERTY). System entities are
-- classified via the System Type relation (88b3d6ad-..., system-minted /
-- non-user-editable), so they returned no typeIds and could not be identified
-- or filtered through the field. Widen the relation type filter to include
-- both, so user-defined and system-defined classifications surface together.
CREATE OR REPLACE FUNCTION public.entities_type_ids(entities entities) RETURNS uuid[] AS $$
  SELECT array_agg(DISTINCT e.id)
  FROM entities e
  INNER JOIN relations r ON e.id = r.to_entity_id
  WHERE r.from_entity_id = entities.id
    AND r.type_id IN (
      '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1',  -- TYPES_PROPERTY
      '88b3d6ad-288c-529c-a212-0e1c24819185'   -- System Type (SYSTEM_TYPES_RELATION_TYPE_ID)
    );
$$ LANGUAGE sql STABLE;
