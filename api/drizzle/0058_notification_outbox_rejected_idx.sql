-- Backs notification-indexer's rejection poller (find_expired_proposals):
-- the LEFT JOIN ... ON (payload->>'proposal_id')::uuid = p.id WHERE o.id IS NULL
-- otherwise sequentially scans the whole (unbounded) outbox every poll.
CREATE INDEX IF NOT EXISTS "idx_outbox_rejected_proposal"
    ON "notification_outbox" (((payload->>'proposal_id')::uuid))
    WHERE event_type = 'proposal_rejected';
