CREATE TABLE atlas_checkpoints (
    indexer_id TEXT PRIMARY KEY CHECK (char_length(indexer_id) > 0),
    cursor TEXT NOT NULL CHECK (char_length(cursor) > 0),
    block_number BIGINT NOT NULL CHECK (block_number >= 0),
    graph_state_version INTEGER NOT NULL CHECK (graph_state_version > 0),
    runtime_compatibility_marker TEXT NOT NULL CHECK (char_length(runtime_compatibility_marker) > 0),
    root_space_id TEXT NOT NULL CHECK (char_length(root_space_id) > 0),
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    graph_state_blob JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
