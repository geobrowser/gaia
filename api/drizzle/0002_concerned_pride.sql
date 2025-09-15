CREATE TABLE "raw_actions" (
	"id" serial PRIMARY KEY NOT NULL,
	"action_type" bigint NOT NULL,
	"action_version" bigint NOT NULL,
	"sender" varchar(42) NOT NULL,
	"entity" uuid NOT NULL,
	"group_id" uuid,
	"space_pov" varchar(42) NOT NULL,
	"metadata" "bytea",
	"block_number" bigint NOT NULL,
	"block_timestamp" timestamp with time zone NOT NULL,
	"tx_hash" varchar(66) NOT NULL
);
--> statement-breakpoint
CREATE TABLE "subspaces" (
	"parent_space_id" uuid NOT NULL,
	"child_space_id" uuid NOT NULL,
	CONSTRAINT "subspaces_parent_space_id_child_space_id_pk" PRIMARY KEY("parent_space_id","child_space_id")
);
--> statement-breakpoint
CREATE TABLE "user_votes" (
	"id" serial PRIMARY KEY NOT NULL,
	"user_id" varchar(42) NOT NULL,
	"entity_id" uuid NOT NULL,
	"space_id" varchar(42) NOT NULL,
	"vote_type" smallint NOT NULL,
	"voted_at" timestamp with time zone NOT NULL,
	CONSTRAINT "user_votes_user_entity_space_unique" UNIQUE("user_id","entity_id","space_id")
);
--> statement-breakpoint
CREATE TABLE "votes_count" (
	"id" serial PRIMARY KEY NOT NULL,
	"entity_id" uuid NOT NULL,
	"space_id" varchar(42) NOT NULL,
	"upvotes" bigint DEFAULT 0 NOT NULL,
	"downvotes" bigint DEFAULT 0 NOT NULL,
	CONSTRAINT "votes_count_entity_space_unique" UNIQUE("entity_id","space_id")
);
--> statement-breakpoint
ALTER TABLE "values" RENAME COLUMN "value" TO "string";--> statement-breakpoint
ALTER TABLE "values" ALTER COLUMN "string" DROP NOT NULL;--> statement-breakpoint
ALTER TABLE "properties" ALTER COLUMN "type" SET DATA TYPE text;--> statement-breakpoint
DROP TYPE "public"."dataTypes";--> statement-breakpoint
CREATE TYPE "public"."dataTypes" AS ENUM('String', 'Number', 'Boolean', 'Time', 'Point', 'Relation');--> statement-breakpoint
ALTER TABLE "properties" ALTER COLUMN "type" SET DATA TYPE "public"."dataTypes" USING "type"::"public"."dataTypes";--> statement-breakpoint
DROP INDEX "values_text_idx";--> statement-breakpoint
DROP INDEX "values_space_text_idx";--> statement-breakpoint
ALTER TABLE "values" ADD COLUMN "boolean" boolean;--> statement-breakpoint
ALTER TABLE "values" ADD COLUMN "number" numeric;--> statement-breakpoint
ALTER TABLE "values" ADD COLUMN "point" text;--> statement-breakpoint
ALTER TABLE "values" ADD COLUMN "time" text;--> statement-breakpoint
ALTER TABLE "subspaces" ADD CONSTRAINT "subspaces_parent_space_id_spaces_id_fk" FOREIGN KEY ("parent_space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "subspaces" ADD CONSTRAINT "subspaces_child_space_id_spaces_id_fk" FOREIGN KEY ("child_space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "subspaces_parent_space_id_idx" ON "subspaces" USING btree ("parent_space_id");--> statement-breakpoint
CREATE INDEX "subspaces_child_space_id_idx" ON "subspaces" USING btree ("child_space_id");--> statement-breakpoint
CREATE INDEX "idx_user_votes_user_entity_space" ON "user_votes" USING btree ("user_id","entity_id","space_id");--> statement-breakpoint
CREATE INDEX "idx_votes_count_space" ON "votes_count" USING btree ("space_id");--> statement-breakpoint
CREATE INDEX "idx_votes_count_entity_space" ON "votes_count" USING btree ("entity_id","space_id");--> statement-breakpoint
ALTER TABLE "editors" ADD CONSTRAINT "editors_space_id_spaces_id_fk" FOREIGN KEY ("space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "members" ADD CONSTRAINT "members_space_id_spaces_id_fk" FOREIGN KEY ("space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "relations" ADD CONSTRAINT "relations_entity_id_entities_id_fk" FOREIGN KEY ("entity_id") REFERENCES "public"."entities"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "relations" ADD CONSTRAINT "relations_type_id_properties_id_fk" FOREIGN KEY ("type_id") REFERENCES "public"."properties"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "relations" ADD CONSTRAINT "relations_from_entity_id_entities_id_fk" FOREIGN KEY ("from_entity_id") REFERENCES "public"."entities"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "relations" ADD CONSTRAINT "relations_from_space_id_spaces_id_fk" FOREIGN KEY ("from_space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "relations" ADD CONSTRAINT "relations_to_entity_id_entities_id_fk" FOREIGN KEY ("to_entity_id") REFERENCES "public"."entities"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "relations" ADD CONSTRAINT "relations_to_space_id_spaces_id_fk" FOREIGN KEY ("to_space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "relations" ADD CONSTRAINT "relations_space_id_spaces_id_fk" FOREIGN KEY ("space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "values" ADD CONSTRAINT "values_property_id_properties_id_fk" FOREIGN KEY ("property_id") REFERENCES "public"."properties"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "values" ADD CONSTRAINT "values_entity_id_entities_id_fk" FOREIGN KEY ("entity_id") REFERENCES "public"."entities"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "values" ADD CONSTRAINT "values_space_id_spaces_id_fk" FOREIGN KEY ("space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "editors_space_id_idx" ON "editors" USING btree ("space_id");--> statement-breakpoint
CREATE INDEX "entities_updated_at_idx" ON "entities" USING btree ("updated_at");--> statement-breakpoint
CREATE INDEX "entities_updated_at_id_idx" ON "entities" USING btree ("updated_at","id");--> statement-breakpoint
CREATE INDEX "members_space_id_idx" ON "members" USING btree ("space_id");--> statement-breakpoint
CREATE INDEX "values_number_idx" ON "values" USING btree ("number");--> statement-breakpoint
CREATE INDEX "values_point_idx" ON "values" USING btree ("point");--> statement-breakpoint
CREATE INDEX "values_boolean_idx" ON "values" USING btree ("boolean");--> statement-breakpoint
CREATE INDEX "values_time_idx" ON "values" USING btree ("time");--> statement-breakpoint
CREATE INDEX "values_text_idx" ON "values" USING btree ("string");--> statement-breakpoint
CREATE INDEX "values_space_text_idx" ON "values" USING btree ("space_id","string");