-- Minimal schema for notification service e2e tests.
-- Only includes tables needed by the notification-indexer and delivery-worker.

-- Required by notification-indexer for rejection polling (expired proposals).
-- Simplified version of the full proposals table — no FK to spaces.
CREATE TABLE IF NOT EXISTS proposals (
    id uuid PRIMARY KEY,
    space_id uuid NOT NULL,
    proposed_by uuid NOT NULL,
    voting_mode text NOT NULL DEFAULT 'Slow',
    start_time bigint NOT NULL,
    end_time bigint NOT NULL,
    quorum bigint NOT NULL DEFAULT 0,
    threshold bigint NOT NULL DEFAULT 0,
    executed_at bigint,
    created_at text NOT NULL DEFAULT '0',
    created_at_block text NOT NULL DEFAULT '0',
    name text,
    yes_count bigint NOT NULL DEFAULT 0,
    no_count bigint NOT NULL DEFAULT 0,
    abstain_count bigint NOT NULL DEFAULT 0
);

-- Required by notification-indexer for per-editor fan-out.
-- Simplified version of the full editors table — no FK to spaces.
CREATE TABLE IF NOT EXISTS editors (
    member_space_id uuid NOT NULL,
    space_id uuid NOT NULL,
    PRIMARY KEY (member_space_id, space_id)
);
CREATE INDEX IF NOT EXISTS editors_space_id_idx ON editors (space_id);

-- Notification service tables (matches migration 0050)
CREATE TABLE IF NOT EXISTS app_webhooks (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    app_name text NOT NULL UNIQUE,
    url text NOT NULL,
    secret text NOT NULL,
    created_at timestamptz DEFAULT now() NOT NULL,
    updated_at timestamptz DEFAULT now() NOT NULL
);

CREATE TABLE IF NOT EXISTS notification_outbox (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    idempotency_key text NOT NULL UNIQUE,
    event_type text NOT NULL,
    payload jsonb NOT NULL,
    created_at timestamptz DEFAULT now() NOT NULL,
    updated_at timestamptz DEFAULT now() NOT NULL
);

CREATE TABLE IF NOT EXISTS notification_deliveries (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    outbox_id uuid NOT NULL REFERENCES notification_outbox(id),
    webhook_id uuid NOT NULL REFERENCES app_webhooks(id),
    status text NOT NULL DEFAULT 'pending',
    attempts smallint NOT NULL DEFAULT 0,
    last_error text,
    next_retry_at timestamptz DEFAULT now() NOT NULL,
    delivered_at timestamptz,
    created_at timestamptz DEFAULT now() NOT NULL,
    updated_at timestamptz DEFAULT now() NOT NULL,
    UNIQUE(outbox_id, webhook_id)
);

CREATE INDEX IF NOT EXISTS idx_deliveries_pending
    ON notification_deliveries (status, next_retry_at);

-- Minimal values table for name enrichment (matches kg-indexer schema).
-- Only includes columns needed by notification-indexer lookups.
CREATE TABLE IF NOT EXISTS "values" (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    entity_id uuid NOT NULL,
    property_id uuid NOT NULL,
    space_id uuid NOT NULL,
    text text,
    UNIQUE(entity_id, property_id, space_id)
);

-- Expression index for the rejection poller's LEFT JOIN anti-pattern.
-- Without this, the (payload->>'proposal_id')::uuid extraction does a seq scan.
CREATE INDEX IF NOT EXISTS idx_outbox_rejected_proposal
    ON notification_outbox (((payload->>'proposal_id')::uuid))
    WHERE event_type = 'proposal_rejected';
