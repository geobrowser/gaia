-- Custom SQL migration file, put your code below! --

-- Expose System Type relations as a dedicated entities.systemTypeIds field.
--
-- System entities are classified through the System Type relation
-- (88b3d6ad-..., system-minted and non-user-editable: the indexer always drops
-- user attempts to author it). This is kept separate from entities.typeIds
-- (which aggregates user-authored Type relations) to preserve provenance — a
-- user can author a regular Type relation pointing at a system-type entity, but
-- cannot forge a System Type relation, so systemTypeIds reliably identifies
-- system entities. PostGraphile exposes this computed column as the
-- `systemTypeIds` field on every Entity.
CREATE OR REPLACE FUNCTION public.entities_system_type_ids(entities entities) RETURNS uuid[] AS $$
  SELECT array_agg(DISTINCT e.id)
  FROM entities e
  INNER JOIN relations r ON e.id = r.to_entity_id
  WHERE r.from_entity_id = entities.id
    AND r.type_id = '88b3d6ad-288c-529c-a212-0e1c24819185';  -- System Type (SYSTEM_TYPES_RELATION_TYPE_ID)
$$ LANGUAGE sql STABLE;
