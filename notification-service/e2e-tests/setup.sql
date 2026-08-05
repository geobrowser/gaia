-- Minimal schema for notification service e2e tests.
-- Only includes tables needed by the notification-indexer and delivery-worker.

-- Required by notification-indexer for rejection polling (expired proposals).
-- Simplified version of the real tables — no FK to spaces.
--
-- Mirrors the governance-v2 split (migration 0067): `proposals` is an identity
-- table and all mutable, version-scoped state lives in `proposal_versions`,
-- with the `proposals_current` view joining each proposal to its current
-- version. The harness previously carried the pre-v2 flat shape, so
-- `find_expired_proposals` — which correctly reads `proposals_current` — failed
-- every poll with `relation "proposals_current" does not exist`, and the
-- proposal_rejected notifications it drives never fired.
CREATE TABLE IF NOT EXISTS proposals (
    id uuid PRIMARY KEY,
    space_id uuid NOT NULL,
    proposed_by uuid NOT NULL,
    created_at text NOT NULL DEFAULT '0',
    created_at_block text NOT NULL DEFAULT '0',
    current_version integer NOT NULL DEFAULT 1,
    executed_at bigint
);

CREATE TABLE IF NOT EXISTS proposal_versions (
    proposal_id uuid NOT NULL,
    proposal_version integer NOT NULL,
    voting_mode text NOT NULL DEFAULT 'Slow',
    start_time bigint NOT NULL,
    end_time bigint NOT NULL,
    quorum bigint NOT NULL DEFAULT 0,
    threshold bigint NOT NULL DEFAULT 0,
    name text,
    yes_count bigint NOT NULL DEFAULT 0,
    no_count bigint NOT NULL DEFAULT 0,
    abstain_count bigint NOT NULL DEFAULT 0,
    PRIMARY KEY (proposal_id, proposal_version)
);

-- Column list mirrors the production view in 0067_governance_v2 for the columns
-- this harness exercises. `executed_at` stays on `proposals` and is carried
-- through, which is what lets find_expired_proposals filter on it.
CREATE OR REPLACE VIEW proposals_current AS
SELECT
    p.id, p.space_id, p.proposed_by, p.created_at, p.created_at_block,
    p.current_version, p.executed_at,
    pv.proposal_version, pv.voting_mode, pv.start_time, pv.end_time,
    pv.quorum, pv.threshold, pv.name,
    pv.yes_count, pv.no_count, pv.abstain_count
FROM proposals p
INNER JOIN proposal_versions pv
    ON pv.proposal_id = p.id
   AND pv.proposal_version = p.current_version;

-- Required by notification-indexer for per-editor fan-out.
-- Simplified version of the full editors table — no FK to spaces.
CREATE TABLE IF NOT EXISTS editors (
    member_space_id uuid NOT NULL,
    space_id uuid NOT NULL,
    PRIMARY KEY (member_space_id, space_id)
);
CREATE INDEX IF NOT EXISTS editors_space_id_idx ON editors (space_id);

-- Required by notification-indexer's proposal-comment membership gate
-- (is_member_or_editor). Simplified version of the full members table.
CREATE TABLE IF NOT EXISTS members (
    member_space_id uuid NOT NULL,
    space_id uuid NOT NULL,
    PRIMARY KEY (member_space_id, space_id)
);
CREATE INDEX IF NOT EXISTS members_space_id_idx ON members (space_id);

-- Required by notification-indexer to resolve prior voters of a proposal
-- (recipients of "a new version of a proposal you voted on was submitted").
-- Simplified version of the full proposal_votes table. voter_id is the voter's
-- personal-space UUID, usable directly as a notification recipient.
CREATE TABLE IF NOT EXISTS proposal_votes (
    proposal_id uuid NOT NULL,
    voter_id uuid NOT NULL,
    space_id uuid,
    vote text,
    created_at text NOT NULL DEFAULT '0',
    created_at_block text NOT NULL DEFAULT '0',
    PRIMARY KEY (proposal_id, voter_id)
);
CREATE INDEX IF NOT EXISTS proposal_votes_proposal_id_idx ON proposal_votes (proposal_id);

-- Notification service tables (matches migration 0050)
CREATE TABLE IF NOT EXISTS app_webhooks (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    app_name text NOT NULL UNIQUE,
    url text NOT NULL,
    secret text NOT NULL,
    notification_types text[],
    space_ids uuid[],
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

-- Minimal relations table for bounty entity→space resolution.
-- Used by lookup_entity_space() and lookup_bounty_space().
CREATE TABLE IF NOT EXISTS relations (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    from_entity_id uuid NOT NULL,
    to_entity_id uuid NOT NULL,
    type_id uuid NOT NULL,
    space_id uuid NOT NULL
);
CREATE INDEX IF NOT EXISTS relations_from_entity_idx ON relations (from_entity_id);

-- Minimal spaces table for bounty entity→space resolution.
-- Used by lookup_entity_space() to filter personal spaces.
CREATE TABLE IF NOT EXISTS spaces (
    id uuid PRIMARY KEY,
    type text NOT NULL DEFAULT 'Personal'
);

-- Expression index for the rejection poller's LEFT JOIN anti-pattern.
-- Without this, the (payload->>'proposal_id')::uuid extraction does a seq scan.
CREATE INDEX IF NOT EXISTS idx_outbox_rejected_proposal
    ON notification_outbox (((payload->>'proposal_id')::uuid))
    WHERE event_type = 'proposal_rejected';

-- Vote aggregate table for the entity-vote-threshold poller (matches migration 0018
-- + 0059). updated_at drives the poller's keyset scan.
CREATE TABLE IF NOT EXISTS votes_count (
    id serial PRIMARY KEY,
    object_id uuid NOT NULL,
    object_type smallint NOT NULL,
    space_id uuid NOT NULL,
    upvotes bigint NOT NULL DEFAULT 0,
    downvotes bigint NOT NULL DEFAULT 0,
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(object_id, object_type, space_id)
);
CREATE INDEX IF NOT EXISTS idx_votes_count_updated_at
    ON votes_count (updated_at, id) WHERE object_type = 0;

-- Persistent keyset cursors for the notification-indexer's pollers (vote poller).
CREATE TABLE IF NOT EXISTS notification_poll_cursors (
    name text PRIMARY KEY,
    cursor_updated_at timestamptz NOT NULL,
    cursor_id bigint NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);

-- Anti-check index for the vote poller's "already notified?" guard (matches migration 0060).
CREATE INDEX IF NOT EXISTS idx_outbox_entity_votes_threshold
    ON notification_outbox (((payload->>'entity_id')::uuid), ((payload->>'vote_space_id')::uuid))
    WHERE event_type = 'entity_votes_threshold';
