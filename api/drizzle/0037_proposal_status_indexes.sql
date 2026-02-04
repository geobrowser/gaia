-- Add indexes to support status filtering on proposals
-- These indexes optimize the WHERE clauses in listProposalsInSpace for status filtering

-- Index for ACCEPTED status filter: executed_at IS NOT NULL
-- Also useful for general queries filtering by execution status
CREATE INDEX "proposals_space_executed_at_idx" ON "proposals" USING btree ("space_id", "executed_at");

-- Composite index for EXECUTABLE/REJECTED/PROPOSED status filtering
-- Covers the columns used in status computation: voting_mode, end_time, yes_count, threshold, quorum
-- Partial index excluding already-executed proposals (most status queries exclude ACCEPTED)
CREATE INDEX "proposals_space_status_filter_idx" ON "proposals" USING btree (
  "space_id",
  "voting_mode",
  "end_time",
  "yes_count",
  "threshold",
  "quorum"
) WHERE "executed_at" IS NULL;

-- Index for queue depth monitoring on proposal_tally_queue
CREATE INDEX "proposal_tally_queue_queued_at_idx" ON "proposal_tally_queue" USING btree ("queued_at");
