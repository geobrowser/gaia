CREATE SCHEMA "ranks";
--> statement-breakpoint
CREATE TABLE "ranks"."ranking_blocks" (
	"id" uuid NOT NULL,
	"space_id" uuid NOT NULL,
	"name" text,
	"filter" text,
	"start_date" timestamp with time zone,
	"end_date" timestamp with time zone,
	"restriction_id" uuid,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "ranking_blocks_id_space_id_pk" PRIMARY KEY("id","space_id")
);
--> statement-breakpoint
CREATE TABLE "ranks"."ranking_items" (
	"ranking_id" uuid NOT NULL,
	"entity_id" uuid NOT NULL,
	"space_id" uuid NOT NULL,
	"position" text,
	"weight" double precision,
	CONSTRAINT "ranking_items_ranking_id_entity_id_space_id_pk" PRIMARY KEY("ranking_id","entity_id","space_id")
);
--> statement-breakpoint
CREATE TABLE "ranks"."ranking_scores" (
	"block_id" uuid NOT NULL,
	"entity_id" uuid NOT NULL,
	"space_id" uuid NOT NULL,
	"score" double precision NOT NULL,
	"position" integer NOT NULL,
	CONSTRAINT "ranking_scores_block_id_entity_id_space_id_pk" PRIMARY KEY("block_id","entity_id","space_id")
);
--> statement-breakpoint
CREATE TABLE "ranks"."rankings" (
	"id" uuid NOT NULL,
	"block_id" uuid,
	"space_id" uuid NOT NULL,
	"author_address" text,
	"rank_type" text,
	"submitted_at" timestamp with time zone,
	"updated_at_block" bigint DEFAULT 0 NOT NULL,
	"update_index" bigint DEFAULT 0 NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "rankings_id_space_id_pk" PRIMARY KEY("id","space_id")
);
--> statement-breakpoint
CREATE INDEX "ranking_scores_block_position_idx" ON "ranks"."ranking_scores" USING btree ("block_id","position");--> statement-breakpoint
CREATE INDEX "rankings_block_id_idx" ON "ranks"."rankings" USING btree ("block_id");--> statement-breakpoint
CREATE INDEX "rankings_block_space_idx" ON "ranks"."rankings" USING btree ("block_id","space_id");