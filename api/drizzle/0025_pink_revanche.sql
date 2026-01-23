CREATE TABLE "edit_versions" (
	"edit_id" uuid PRIMARY KEY NOT NULL,
	"block_number" bigint NOT NULL,
	"sequence" bigint NOT NULL,
	"version_key" bigint NOT NULL,
	"created_at" timestamp NOT NULL,
	CONSTRAINT "edit_versions_block_sequence_unique" UNIQUE("block_number","sequence")
);
--> statement-breakpoint
CREATE TABLE "relation_versions" (
	"id" uuid PRIMARY KEY NOT NULL,
	"relation_id" uuid NOT NULL,
	"entity_id" uuid NOT NULL,
	"type_id" uuid NOT NULL,
	"from_entity_id" uuid NOT NULL,
	"from_space_id" uuid,
	"to_entity_id" uuid NOT NULL,
	"to_space_id" uuid,
	"position" text,
	"space_id" uuid NOT NULL,
	"verified" boolean,
	"valid_from_key" bigint NOT NULL,
	"valid_to_key" bigint
);
--> statement-breakpoint
CREATE TABLE "value_versions" (
	"id" uuid PRIMARY KEY NOT NULL,
	"entity_id" uuid NOT NULL,
	"property_id" uuid NOT NULL,
	"space_id" uuid NOT NULL,
	"valid_from_key" bigint NOT NULL,
	"valid_to_key" bigint,
	"language" text,
	"unit" text,
	"string" text,
	"boolean" boolean,
	"number" numeric,
	"time" text,
	"point" text,
	"integer" bigint,
	"float" double precision,
	"bytes" "bytea",
	"date" text,
	"datetime" text,
	"schedule" jsonb,
	"embedding" jsonb
);
--> statement-breakpoint
CREATE INDEX "edit_versions_version_key_idx" ON "edit_versions" USING btree ("version_key");--> statement-breakpoint
CREATE INDEX "relation_versions_relation_idx" ON "relation_versions" USING btree ("relation_id");--> statement-breakpoint
CREATE INDEX "relation_versions_entity_idx" ON "relation_versions" USING btree ("entity_id");--> statement-breakpoint
CREATE INDEX "relation_versions_open_idx" ON "relation_versions" USING btree ("relation_id") WHERE "relation_versions"."valid_to_key" IS NULL;--> statement-breakpoint
CREATE INDEX "relation_versions_range_idx" ON "relation_versions" USING btree ("entity_id","valid_from_key");--> statement-breakpoint
CREATE INDEX "value_versions_entity_idx" ON "value_versions" USING btree ("entity_id");--> statement-breakpoint
CREATE INDEX "value_versions_lookup_idx" ON "value_versions" USING btree ("entity_id","property_id","space_id");--> statement-breakpoint
CREATE INDEX "value_versions_open_idx" ON "value_versions" USING btree ("entity_id","property_id","space_id") WHERE "value_versions"."valid_to_key" IS NULL;--> statement-breakpoint
CREATE INDEX "value_versions_range_idx" ON "value_versions" USING btree ("entity_id","valid_from_key");