CREATE TYPE "public"."dataTypes" AS ENUM('Text', 'Number', 'Checkbox', 'Time', 'Point', 'Relation');--> statement-breakpoint
CREATE TYPE "public"."spaceTypes" AS ENUM('Personal', 'Public');--> statement-breakpoint
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
	"space_id" text NOT NULL,
	"value" text NOT NULL,
	"language" text,
	"unit" text
);
--> statement-breakpoint
CREATE INDEX "values_text_idx" ON "values" USING btree ("value");--> statement-breakpoint
CREATE INDEX "values_space_text_idx" ON "values" USING btree ("space_id","value");