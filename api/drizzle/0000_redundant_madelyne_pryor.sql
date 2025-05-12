CREATE TABLE "entities" (
	"id" text PRIMARY KEY NOT NULL,
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
	"space" text NOT NULL,
	CONSTRAINT "ipfs_cache_uri_unique" UNIQUE("uri")
);
--> statement-breakpoint
CREATE TABLE "triples" (
	"id" text PRIMARY KEY NOT NULL,
	"attribute_id" text NOT NULL,
	"entity_id" text NOT NULL,
	"space_id" text NOT NULL,
	"text_value" text,
	"number_value" text,
	"boolean_value" boolean,
	"language_option" text,
	"format_option" text,
	"unit_option" text,
	"value_type" text
);
