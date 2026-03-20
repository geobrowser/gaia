-- Minimal schema for score lookup e2e tests.
-- Only the values table is needed (for entity-space lookups).

CREATE TABLE IF NOT EXISTS values (
    id text PRIMARY KEY,
    entity_id uuid NOT NULL,
    property_id uuid NOT NULL,
    space_id uuid NOT NULL,
    text text
);

CREATE INDEX IF NOT EXISTS values_entity_space_idx ON values (entity_id, space_id);
CREATE INDEX IF NOT EXISTS values_space_idx ON values (space_id);
