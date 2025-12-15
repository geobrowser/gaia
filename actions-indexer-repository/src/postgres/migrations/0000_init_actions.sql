CREATE TABLE raw_actions (
    id              SERIAL PRIMARY KEY,
    action_type     BIGINT NOT NULL,
    action_version  BIGINT NOT NULL,
    sender          VARCHAR(42) NOT NULL,
    object_id       UUID NOT NULL,
    group_id        UUID,
    space_pov       UUID NOT NULL,
    metadata        BYTEA,
    block_number    BIGINT NOT NULL,
    block_timestamp TIMESTAMPTZ NOT NULL,
    tx_hash         VARCHAR(66) NOT NULL,
    object_type     SMALLINT NOT NULL
);

CREATE TABLE user_votes (
    id              SERIAL PRIMARY KEY,
    user_id         VARCHAR(42) NOT NULL,
    object_id       UUID NOT NULL,
    space_id        UUID NOT NULL,
    vote_type       SMALLINT NOT NULL,
    voted_at        TIMESTAMPTZ NOT NULL,
    object_type     SMALLINT NOT NULL,
    UNIQUE(user_id, object_id, object_type, space_id)
);

CREATE TABLE votes_count (
    id              SERIAL PRIMARY KEY,
    object_id       UUID NOT NULL,
    space_id        UUID NOT NULL,
    upvotes         BIGINT NOT NULL DEFAULT 0,
    downvotes       BIGINT NOT NULL DEFAULT 0,
    object_type     SMALLINT NOT NULL,
    UNIQUE(object_id, object_type, space_id)
);

CREATE TABLE meta (
    id              VARCHAR(255) PRIMARY KEY,
    cursor          TEXT NOT NULL,
    block_number    TEXT NOT NULL
);

CREATE INDEX idx_user_votes_user_object_space ON user_votes(user_id, object_id, object_type, space_id);
CREATE INDEX idx_votes_count_space ON votes_count(space_id);
CREATE INDEX idx_votes_count_object_space ON votes_count(object_id, object_type, space_id);
CREATE INDEX idx_meta_cursor ON meta(cursor);