ALTER TABLE "editors" DROP CONSTRAINT "editors_space_id_spaces_id_fk";
--> statement-breakpoint
ALTER TABLE "members" DROP CONSTRAINT "members_space_id_spaces_id_fk";
--> statement-breakpoint
ALTER TABLE "relations" DROP CONSTRAINT "relations_entity_id_entities_id_fk";
--> statement-breakpoint
ALTER TABLE "relations" DROP CONSTRAINT "relations_type_id_properties_id_fk";
--> statement-breakpoint
ALTER TABLE "relations" DROP CONSTRAINT "relations_from_entity_id_entities_id_fk";
--> statement-breakpoint
ALTER TABLE "relations" DROP CONSTRAINT "relations_from_space_id_spaces_id_fk";
--> statement-breakpoint
ALTER TABLE "relations" DROP CONSTRAINT "relations_to_entity_id_entities_id_fk";
--> statement-breakpoint
ALTER TABLE "relations" DROP CONSTRAINT "relations_to_space_id_spaces_id_fk";
--> statement-breakpoint
ALTER TABLE "relations" DROP CONSTRAINT "relations_space_id_spaces_id_fk";
--> statement-breakpoint
ALTER TABLE "subspaces" DROP CONSTRAINT "subspaces_parent_space_id_spaces_id_fk";
--> statement-breakpoint
ALTER TABLE "subspaces" DROP CONSTRAINT "subspaces_child_space_id_spaces_id_fk";
--> statement-breakpoint
ALTER TABLE "values" DROP CONSTRAINT "values_property_id_properties_id_fk";
--> statement-breakpoint
ALTER TABLE "values" DROP CONSTRAINT "values_entity_id_entities_id_fk";
--> statement-breakpoint
ALTER TABLE "values" DROP CONSTRAINT "values_space_id_spaces_id_fk";
--> statement-breakpoint
-- Re-enable triggers now that FK constraints are removed
ALTER TABLE "spaces" ENABLE TRIGGER USER;
--> statement-breakpoint
ALTER TABLE "entities" ENABLE TRIGGER USER;
--> statement-breakpoint
ALTER TABLE "properties" ENABLE TRIGGER USER;
--> statement-breakpoint
ALTER TABLE "values" ENABLE TRIGGER USER;
--> statement-breakpoint
ALTER TABLE "relations" ENABLE TRIGGER USER;
--> statement-breakpoint
ALTER TABLE "members" ENABLE TRIGGER USER;
--> statement-breakpoint
ALTER TABLE "editors" ENABLE TRIGGER USER;
--> statement-breakpoint
-- PostGraphile smart comments to define relationships without FK enforcement
-- These replace the COMMENT ON CONSTRAINT statements from 0004_functions.sql
COMMENT ON TABLE "values" IS E'@foreignKey (property_id) references properties (id)|@fieldName property|@omit many\n@foreignKey (entity_id) references entities (id)|@fieldName entity|@foreignFieldName values|@foreignSimpleFieldName valuesList\n@foreignKey (space_id) references spaces (id)|@fieldName space';
--> statement-breakpoint
COMMENT ON TABLE "relations" IS E'@foreignKey (type_id) references properties (id)|@fieldName type|@omit many\n@foreignKey (entity_id) references entities (id)|@fieldName entity|@foreignFieldName relationsWhereEntity|@foreignSimpleFieldName relationsWhereEntityList\n@foreignKey (from_entity_id) references entities (id)|@fieldName fromEntity|@foreignFieldName relations|@foreignSimpleFieldName relationsList\n@foreignKey (to_entity_id) references entities (id)|@fieldName toEntity|@foreignFieldName backlinks|@foreignSimpleFieldName backlinksList\n@foreignKey (space_id) references spaces (id)|@fieldName space\n@foreignKey (from_space_id) references spaces (id)|@fieldName fromSpace\n@foreignKey (to_space_id) references spaces (id)|@fieldName toSpace';
--> statement-breakpoint
COMMENT ON TABLE "members" IS E'@foreignKey (space_id) references spaces (id)|@fieldName space|@foreignFieldName members|@foreignSimpleFieldName membersList';
--> statement-breakpoint
COMMENT ON TABLE "editors" IS E'@foreignKey (space_id) references spaces (id)|@fieldName space|@foreignFieldName editors|@foreignSimpleFieldName editorsList';
