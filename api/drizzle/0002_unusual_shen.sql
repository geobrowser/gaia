-- Custom SQL migration file, put your code below! --
CREATE OR REPLACE FUNCTION public.entity_name(entity entities) RETURNS text AS $$
  SELECT value FROM values WHERE entity_id = entity.id AND property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935' LIMIT 1;
$$ LANGUAGE sql STABLE;

COMMENT ON FUNCTION public.entity_name(entities) IS '@filterable';

CREATE OR REPLACE FUNCTION public.entity_description(entity entities) RETURNS text AS $$
  SELECT value FROM values WHERE entity_id = entity.id AND property_id = '9b1f76ff-9711-404c-861e-59dc3fa7d037' LIMIT 1;
$$ LANGUAGE sql STABLE;

COMMENT ON FUNCTION public.entity_description(entities) IS '@filterable';

CREATE OR REPLACE FUNCTION public.entity_types(entity entities) RETURNS SETOF public.entities AS $$
  SELECT e.*
  FROM entities e
  INNER JOIN relations r ON e.id = r.to_entity_id
  WHERE r.from_entity_id = entity.id AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1';
$$ LANGUAGE sql STABLE;


CREATE OR REPLACE FUNCTION public.entity_values(entity entities) RETURNS SETOF public.values AS $$
  SELECT v.*
  FROM values v
  WHERE v.entity_id = entity.id
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION public.entity_relations(entity entities) RETURNS SETOF public.relations AS $$
  SELECT r.*
  FROM relations r
  WHERE r.from_entity_id = entity.id
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION public.entity_backlinks(entity entities) RETURNS SETOF public.relations AS $$
  SELECT r.*
  FROM relations r
  WHERE r.to_entity_id = entity.id
$$ LANGUAGE sql STABLE;

-- COMMENT ON FUNCTION public.entity_types(entities) IS '@filterable';
-- COMMENT ON FUNCTION public.entity_values(entities) IS '@filterable';
-- COMMENT ON FUNCTION public.entity_relations(entities) IS '@filterable';
-- COMMENT ON FUNCTION public.entity_backlinks(entities) IS '@filterable';
