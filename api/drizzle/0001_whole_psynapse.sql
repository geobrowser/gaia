-- Custom SQL migration file, put your code below! --
CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- Add GIN index with trigram operations for efficient text similarity searches
CREATE INDEX IF NOT EXISTS values_text_gin_trgm_idx ON values USING GIN (value gin_trgm_ops);
