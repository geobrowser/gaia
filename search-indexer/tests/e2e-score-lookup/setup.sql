-- Minimal schema for Postgres lookup e2e tests.
-- The values table is used for entity-space lookups (scores, space topics, topology).
-- The relations table is used for relation-to-doc lookups (RemoveRelationById).

CREATE TABLE IF NOT EXISTS values (
    id text PRIMARY KEY,
    entity_id uuid NOT NULL,
    property_id uuid NOT NULL,
    space_id uuid NOT NULL,
    text text
);

CREATE INDEX IF NOT EXISTS values_entity_space_idx ON values (entity_id, space_id);
CREATE INDEX IF NOT EXISTS values_space_idx ON values (space_id);

CREATE TABLE IF NOT EXISTS relations (
    id uuid PRIMARY KEY,
    entity_id uuid NOT NULL,
    type_id uuid NOT NULL,
    from_entity_id uuid NOT NULL,
    from_space_id uuid,
    to_entity_id uuid NOT NULL,
    to_space_id uuid,
    position text,
    space_id uuid NOT NULL,
    verified boolean
);

CREATE INDEX IF NOT EXISTS relations_id_idx ON relations (id);
