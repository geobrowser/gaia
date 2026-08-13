-- Minimal shapes matching production (verified against the live schema).
CREATE TABLE entities (id uuid PRIMARY KEY, created_at text, created_at_block text, updated_at text, updated_at_block text);
CREATE TABLE values (id text PRIMARY KEY, property_id uuid NOT NULL, entity_id uuid NOT NULL, space_id uuid NOT NULL, text text);
CREATE TABLE relations (id uuid PRIMARY KEY, entity_id uuid NOT NULL, type_id uuid NOT NULL, from_entity_id uuid NOT NULL,
                        to_entity_id uuid NOT NULL, space_id uuid NOT NULL, is_system boolean NOT NULL DEFAULT false);
CREATE TABLE votes_count (id serial PRIMARY KEY, object_id uuid NOT NULL, object_type smallint NOT NULL,
                          space_id uuid NOT NULL, vote_kind smallint NOT NULL DEFAULT 0,
                          positive bigint NOT NULL DEFAULT 0, negative bigint NOT NULL DEFAULT 0,
                          updated_at timestamptz NOT NULL DEFAULT now());
