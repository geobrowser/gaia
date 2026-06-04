-- Backs the vote poller's "already notified?" anti-check so it skips entities
-- already notified at the current threshold (avoids re-resolving the creator and
-- re-attempting the insert for entities that keep accruing votes).
CREATE INDEX IF NOT EXISTS "idx_outbox_entity_votes_threshold"
    ON "notification_outbox" (((payload->>'entity_id')::uuid), ((payload->>'vote_space_id')::uuid))
    WHERE event_type = 'entity_votes_threshold';
