CREATE OR REPLACE FUNCTION public.spaces_page(space spaces)
RETURNS public.entities AS $$
  SELECT e.*
  FROM entities e
  WHERE e.id = COALESCE(
    space.topic_id,
    (
      SELECT r.from_entity_id
      FROM relations r
      WHERE r.space_id = space.id
        AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1' -- SystemIds.Types
        AND r.to_entity_id = '362c1dbd-dc64-44bb-a3c4-652f38a642d7' -- SystemIds.SPACE_TYPE
      LIMIT 1
    )
  )
  LIMIT 1;
$$ LANGUAGE sql STABLE;
