ALTER TABLE "editors" DROP CONSTRAINT IF EXISTS "editors_space_id_spaces_id_fk";
ALTER TABLE "editors" ADD CONSTRAINT "editors_space_id_spaces_id_fk" FOREIGN KEY ("space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action NOT VALID;
ALTER TABLE "members" DROP CONSTRAINT IF EXISTS "members_space_id_spaces_id_fk";
ALTER TABLE "members" ADD CONSTRAINT "members_space_id_spaces_id_fk" FOREIGN KEY ("space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action NOT VALID;
ALTER TABLE "relations" DROP CONSTRAINT IF EXISTS "relations_entity_id_entities_id_fk";
ALTER TABLE "relations" ADD CONSTRAINT "relations_entity_id_entities_id_fk" FOREIGN KEY ("entity_id") REFERENCES "public"."entities"("id") ON DELETE no action ON UPDATE no action NOT VALID;
ALTER TABLE "relations" DROP CONSTRAINT IF EXISTS "relations_type_id_properties_id_fk";
ALTER TABLE "relations" ADD CONSTRAINT "relations_type_id_properties_id_fk" FOREIGN KEY ("type_id") REFERENCES "public"."properties"("id") ON DELETE no action ON UPDATE no action NOT VALID;
ALTER TABLE "relations" DROP CONSTRAINT IF EXISTS "relations_from_entity_id_entities_id_fk";
ALTER TABLE "relations" ADD CONSTRAINT "relations_from_entity_id_entities_id_fk" FOREIGN KEY ("from_entity_id") REFERENCES "public"."entities"("id") ON DELETE no action ON UPDATE no action NOT VALID;
ALTER TABLE "relations" DROP CONSTRAINT IF EXISTS "relations_from_space_id_spaces_id_fk";
ALTER TABLE "relations" ADD CONSTRAINT "relations_from_space_id_spaces_id_fk" FOREIGN KEY ("from_space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action NOT VALID;
ALTER TABLE "relations" DROP CONSTRAINT IF EXISTS "relations_to_entity_id_entities_id_fk";
ALTER TABLE "relations" ADD CONSTRAINT "relations_to_entity_id_entities_id_fk" FOREIGN KEY ("to_entity_id") REFERENCES "public"."entities"("id") ON DELETE no action ON UPDATE no action NOT VALID;
ALTER TABLE "relations" DROP CONSTRAINT IF EXISTS "relations_to_space_id_spaces_id_fk";
ALTER TABLE "relations" ADD CONSTRAINT "relations_to_space_id_spaces_id_fk" FOREIGN KEY ("to_space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action NOT VALID;
ALTER TABLE "relations" DROP CONSTRAINT IF EXISTS "relations_space_id_spaces_id_fk";
ALTER TABLE "relations" ADD CONSTRAINT "relations_space_id_spaces_id_fk" FOREIGN KEY ("space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action NOT VALID;
ALTER TABLE "values" DROP CONSTRAINT IF EXISTS "values_property_id_properties_id_fk";
ALTER TABLE "values" ADD CONSTRAINT "values_property_id_properties_id_fk" FOREIGN KEY ("property_id") REFERENCES "public"."properties"("id") ON DELETE no action ON UPDATE no action NOT VALID;
ALTER TABLE "values" DROP CONSTRAINT IF EXISTS "values_entity_id_entities_id_fk";
ALTER TABLE "values" ADD CONSTRAINT "values_entity_id_entities_id_fk" FOREIGN KEY ("entity_id") REFERENCES "public"."entities"("id") ON DELETE no action ON UPDATE no action NOT VALID;
ALTER TABLE "values" DROP CONSTRAINT IF EXISTS "values_space_id_spaces_id_fk";
ALTER TABLE "values" ADD CONSTRAINT "values_space_id_spaces_id_fk" FOREIGN KEY ("space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action NOT VALID;

-- Disable ALL triggers for every table
ALTER TABLE "spaces" DISABLE TRIGGER ALL;
ALTER TABLE "entities" DISABLE TRIGGER ALL;
ALTER TABLE "properties" DISABLE TRIGGER ALL;
ALTER TABLE "values" DISABLE TRIGGER ALL;
ALTER TABLE "relations" DISABLE TRIGGER ALL;
ALTER TABLE "members" DISABLE TRIGGER ALL;
ALTER TABLE "editors" DISABLE TRIGGER ALL;

-- Custom SQL migration file, put your code below! --
CREATE OR REPLACE FUNCTION public.entities_name(entity entities) RETURNS text AS $$
  SELECT value FROM values WHERE entity_id = entity.id AND property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935' LIMIT 1;
$$ LANGUAGE sql STABLE;


CREATE OR REPLACE FUNCTION public.entities_description(entity entities) RETURNS text AS $$
  SELECT value FROM values WHERE entity_id = entity.id AND property_id = '9b1f76ff-9711-404c-861e-59dc3fa7d037' LIMIT 1;
$$ LANGUAGE sql STABLE;


CREATE OR REPLACE FUNCTION public.entities_types(entity entities) RETURNS SETOF public.entities AS $$
  SELECT e.*
  FROM entities e
  INNER JOIN relations r ON e.id = r.to_entity_id
  WHERE r.from_entity_id = entity.id AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1';
$$ LANGUAGE sql STABLE;


CREATE OR REPLACE FUNCTION public.entities_values(entity entities) RETURNS SETOF public.values AS $$
  SELECT v.*
  FROM values v
  WHERE v.entity_id = entity.id
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION public.entities_relations(entity entities) RETURNS SETOF public.relations AS $$
  SELECT r.*
  FROM relations r
  WHERE r.from_entity_id = entity.id
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION public.entities_backlinks(entity entities) RETURNS SETOF public.relations AS $$
  SELECT r.*
  FROM relations r
  WHERE r.to_entity_id = entity.id
$$ LANGUAGE sql STABLE;
