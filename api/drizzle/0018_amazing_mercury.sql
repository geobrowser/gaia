CREATE TABLE "user_votes" (
	"user_id" uuid NOT NULL,
	"object_id" uuid NOT NULL,
	"object_type" smallint NOT NULL,
	"space_id" uuid NOT NULL,
	"vote_type" smallint NOT NULL,
	"voted_at" timestamp with time zone NOT NULL,
	CONSTRAINT "user_votes_user_id_object_id_object_type_space_id_unique" UNIQUE("user_id","object_id","object_type","space_id")
);
--> statement-breakpoint
CREATE TABLE "votes" (
	"id" serial PRIMARY KEY NOT NULL,
	"voter_id" uuid NOT NULL,
	"object_id" uuid NOT NULL,
	"object_type" smallint NOT NULL,
	"space_id" uuid NOT NULL,
	"vote" smallint NOT NULL,
	"block_number" bigint NOT NULL,
	"block_timestamp" timestamp with time zone NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "votes_count" (
	"id" serial PRIMARY KEY NOT NULL,
	"object_id" uuid NOT NULL,
	"object_type" smallint NOT NULL,
	"space_id" uuid NOT NULL,
	"upvotes" bigint DEFAULT 0 NOT NULL,
	"downvotes" bigint DEFAULT 0 NOT NULL,
	CONSTRAINT "votes_count_object_id_object_type_space_id_unique" UNIQUE("object_id","object_type","space_id")
);
--> statement-breakpoint
CREATE INDEX "idx_user_votes_object" ON "user_votes" USING btree ("object_id","object_type","space_id");--> statement-breakpoint
CREATE INDEX "idx_votes_voter_id" ON "votes" USING btree ("voter_id");--> statement-breakpoint
CREATE INDEX "idx_votes_object_type_object_id_space_id" ON "votes" USING btree ("object_type","object_id","space_id");