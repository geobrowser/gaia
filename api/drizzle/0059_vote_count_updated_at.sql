-- Persistent keyset cursors for the notification-indexer's pollers (e.g. the
-- entity-vote-threshold poller).
CREATE TABLE IF NOT EXISTS "notification_poll_cursors" (
	"name" text PRIMARY KEY NOT NULL,
	"cursor_updated_at" timestamp with time zone NOT NULL,
	"cursor_id" bigint NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
-- Backfill existing rows to an old sentinel (epoch) via a CONSTANT default: this
-- uses PG 11+'s fast-default path (no table rewrite, no bulk UPDATE) and keeps the
-- column NOT NULL. Crucially the sentinel is *old*, so existing rows fall outside
-- the vote poller's cold-start lookback window — a deploy won't notify every
-- already-over-threshold entity (backfill storm).
ALTER TABLE "votes_count" ADD COLUMN IF NOT EXISTS "updated_at" timestamp with time zone NOT NULL DEFAULT '1970-01-01 00:00:00+00';
--> statement-breakpoint
-- New rows get the real write time going forward (the vote-indexer also sets
-- clock_timestamp() explicitly on every upsert).
ALTER TABLE "votes_count" ALTER COLUMN "updated_at" SET DEFAULT now();
--> statement-breakpoint
-- The two indexes below are non-CONCURRENTLY + IF NOT EXISTS so a fresh/small DB
-- gets them inline. On a LARGE/prod DB a plain CREATE INDEX write-locks the table
-- for the whole build (stalling the vote-indexer on votes_count and the delivery
-- pipeline on notification_outbox), so run the CONCURRENTLY versions MANUALLY
-- *before* applying this migration — IF NOT EXISTS then makes these no-ops:
--
--   CREATE INDEX CONCURRENTLY IF NOT EXISTS "idx_votes_count_updated_at"
--       ON "votes_count" USING btree ("updated_at", "id") WHERE object_type = 0;
--
--   CREATE INDEX CONCURRENTLY IF NOT EXISTS "idx_outbox_entity_votes_threshold"
--       ON "notification_outbox" (((payload->>'entity_id')::uuid), ((payload->>'vote_space_id')::uuid))
--       WHERE event_type = 'entity_votes_threshold';
CREATE INDEX IF NOT EXISTS "idx_votes_count_updated_at"
	ON "votes_count" USING btree ("updated_at","id") WHERE object_type = 0;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_outbox_entity_votes_threshold"
	ON "notification_outbox" (((payload->>'entity_id')::uuid), ((payload->>'vote_space_id')::uuid))
	WHERE event_type = 'entity_votes_threshold';
