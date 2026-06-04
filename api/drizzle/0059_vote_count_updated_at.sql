CREATE TABLE "notification_poll_cursors" (
	"name" text PRIMARY KEY NOT NULL,
	"cursor_updated_at" timestamp with time zone NOT NULL,
	"cursor_id" bigint NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
ALTER TABLE "votes_count" ADD COLUMN "updated_at" timestamp with time zone DEFAULT now() NOT NULL;--> statement-breakpoint
CREATE INDEX "idx_votes_count_updated_at" ON "votes_count" USING btree ("updated_at","id") WHERE object_type = 0;