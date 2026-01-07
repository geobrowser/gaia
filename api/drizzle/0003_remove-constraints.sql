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

-- Disable user triggers for every table so we can batch write data. We only add the FOREIGN
-- key constraints so that postgraphile can pick up our relational schema.
-- Note: Using USER instead of ALL because ALL requires superuser privileges on managed databases.
ALTER TABLE "spaces" DISABLE TRIGGER USER;
ALTER TABLE "entities" DISABLE TRIGGER USER;
ALTER TABLE "properties" DISABLE TRIGGER USER;
ALTER TABLE "values" DISABLE TRIGGER USER;
ALTER TABLE "relations" DISABLE TRIGGER USER;
ALTER TABLE "members" DISABLE TRIGGER USER;
ALTER TABLE "editors" DISABLE TRIGGER USER;
