-- Persistent keyset cursors for the notification-indexer's pollers (e.g. the
-- entity-vote-threshold poller).
CREATE TABLE IF NOT EXISTS "notification_poll_cursors" (
	"name" text PRIMARY KEY NOT NULL,
	"cursor_updated_at" timestamp with time zone NOT NULL,
	"cursor_id" bigint NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
-- now() is STABLE, so PG 11+ stores a single fast catalog default here (no full
-- table rewrite) — just a brief ACCESS EXCLUSIVE lock. The vote-indexer writes
-- clock_timestamp() explicitly on every upsert, so this default only ever fills
-- the one-time backfill of existing rows.
ALTER TABLE "votes_count" ADD COLUMN IF NOT EXISTS "updated_at" timestamp with time zone DEFAULT now() NOT NULL;
--> statement-breakpoint
-- ─────────────────────────────────────────────────────────────────────────────
-- INDEXES — create these MANUALLY, post-deploy, with CONCURRENTLY.
--
-- A plain CREATE INDEX holds a write-blocking lock for the whole build, which on
-- prod would stall the vote-indexer (votes_count) and, worse, the notification
-- delivery pipeline (notification_outbox can be large). CREATE INDEX CONCURRENTLY
-- avoids that, but it cannot run inside a transaction and drizzle-kit wraps each
-- migration in one — so the index DDL is intentionally left out of the migration.
-- The vote poller is correct without these (just does full scans / extra work);
-- run them by hand before or immediately after rollout:
--
--   CREATE INDEX CONCURRENTLY IF NOT EXISTS "idx_votes_count_updated_at"
--       ON "votes_count" USING btree ("updated_at", "id") WHERE object_type = 0;
--
--   CREATE INDEX CONCURRENTLY IF NOT EXISTS "idx_outbox_entity_votes_threshold"
--       ON "notification_outbox" (((payload->>'entity_id')::uuid), ((payload->>'vote_space_id')::uuid))
--       WHERE event_type = 'entity_votes_threshold';
-- ─────────────────────────────────────────────────────────────────────────────
