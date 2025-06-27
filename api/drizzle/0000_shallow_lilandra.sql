CREATE TYPE "public"."dataTypes" AS ENUM('Text', 'Number', 'Checkbox', 'Time', 'Point', 'Relation');--> statement-breakpoint
CREATE TYPE "public"."spaceTypes" AS ENUM('Personal', 'Public');--> statement-breakpoint
CREATE TABLE "cursors" (
	"id" text PRIMARY KEY NOT NULL,
	"cursor" text NOT NULL,
	"block_number" text NOT NULL
);
--> statement-breakpoint
CREATE TABLE "editors" (
	"address" text NOT NULL,
	"space_id" uuid NOT NULL,
	CONSTRAINT "editors_address_space_id_pk" PRIMARY KEY("address","space_id")
);
--> statement-breakpoint
CREATE TABLE "entities" (
	"id" uuid PRIMARY KEY NOT NULL,
	"created_at" text NOT NULL,
	"created_at_block" text NOT NULL,
	"updated_at" text NOT NULL,
	"updated_at_block" text NOT NULL
);
--> statement-breakpoint
CREATE TABLE "ipfs_cache" (
	"id" serial NOT NULL,
	"json" jsonb,
	"uri" text NOT NULL,
	"is_errored" boolean DEFAULT false NOT NULL,
	"block" text NOT NULL,
	"space" uuid NOT NULL,
	CONSTRAINT "ipfs_cache_uri_unique" UNIQUE("uri")
);
--> statement-breakpoint
CREATE TABLE "members" (
	"address" text NOT NULL,
	"space_id" uuid NOT NULL,
	CONSTRAINT "members_address_space_id_pk" PRIMARY KEY("address","space_id")
);
--> statement-breakpoint
CREATE TABLE "properties" (
	"id" uuid PRIMARY KEY NOT NULL,
	"type" "dataTypes" NOT NULL
);
--> statement-breakpoint
CREATE TABLE "relations" (
	"id" uuid PRIMARY KEY NOT NULL,
	"entity_id" uuid NOT NULL,
	"type_id" uuid NOT NULL,
	"from_entity_id" uuid NOT NULL,
	"from_space_id" uuid,
	"from_version_id" uuid,
	"to_entity_id" uuid NOT NULL,
	"to_space_id" uuid,
	"to_version_id" uuid,
	"position" text,
	"space_id" uuid NOT NULL,
	"verified" boolean
);
--> statement-breakpoint
CREATE TABLE "spaces" (
	"id" uuid PRIMARY KEY NOT NULL,
	"type" "spaceTypes" NOT NULL,
	"dao_address" text NOT NULL,
	"space_address" text NOT NULL,
	"main_voting_address" text,
	"membership_address" text,
	"personal_address" text
);
--> statement-breakpoint
CREATE TABLE "values" (
	"id" text PRIMARY KEY NOT NULL,
	"property_id" uuid NOT NULL,
	"entity_id" uuid NOT NULL,
	"space_id" uuid NOT NULL,
	"value" text NOT NULL,
	"language" text,
	"unit" text
);
--> statement-breakpoint
CREATE INDEX "properties_type_idx" ON "properties" USING btree ("type");--> statement-breakpoint
CREATE INDEX "relations_entity_id_idx" ON "relations" USING btree ("entity_id");--> statement-breakpoint
CREATE INDEX "relations_type_id_idx" ON "relations" USING btree ("type_id");--> statement-breakpoint
CREATE INDEX "relations_from_entity_id_idx" ON "relations" USING btree ("from_entity_id");--> statement-breakpoint
CREATE INDEX "relations_to_entity_id_idx" ON "relations" USING btree ("to_entity_id");--> statement-breakpoint
CREATE INDEX "relations_space_id_idx" ON "relations" USING btree ("space_id");--> statement-breakpoint
CREATE INDEX "relations_space_from_to_idx" ON "relations" USING btree ("space_id","from_entity_id","to_entity_id");--> statement-breakpoint
CREATE INDEX "relations_space_type_idx" ON "relations" USING btree ("space_id","type_id");--> statement-breakpoint
CREATE INDEX "relations_to_entity_space_idx" ON "relations" USING btree ("to_entity_id","space_id");--> statement-breakpoint
CREATE INDEX "relations_from_entity_space_idx" ON "relations" USING btree ("from_entity_id","space_id");--> statement-breakpoint
CREATE INDEX "relations_entity_type_space_idx" ON "relations" USING btree ("entity_id","type_id","space_id");--> statement-breakpoint
CREATE INDEX "relations_type_from_to_idx" ON "relations" USING btree ("type_id","from_entity_id","to_entity_id");--> statement-breakpoint
CREATE INDEX "values_property_id_idx" ON "values" USING btree ("property_id");--> statement-breakpoint
CREATE INDEX "values_entity_id_idx" ON "values" USING btree ("entity_id");--> statement-breakpoint
CREATE INDEX "values_space_id_idx" ON "values" USING btree ("space_id");--> statement-breakpoint
CREATE INDEX "values_text_idx" ON "values" USING btree ("value");--> statement-breakpoint
CREATE INDEX "values_entity_property_idx" ON "values" USING btree ("entity_id","property_id");--> statement-breakpoint
CREATE INDEX "values_entity_space_idx" ON "values" USING btree ("entity_id","space_id");--> statement-breakpoint
CREATE INDEX "values_property_space_idx" ON "values" USING btree ("property_id","space_id");--> statement-breakpoint
CREATE INDEX "values_entity_property_space_idx" ON "values" USING btree ("entity_id","property_id","space_id");--> statement-breakpoint
CREATE INDEX "values_space_text_idx" ON "values" USING btree ("space_id","value");--> statement-breakpoint
CREATE INDEX "values_language_idx" ON "values" USING btree ("language");--> statement-breakpoint
CREATE INDEX "values_unit_idx" ON "values" USING btree ("unit");