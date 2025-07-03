-- Custom SQL migration file, put your code below! --
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE INDEX IF NOT EXISTS values_text_gin_trgm_idx ON values USING GIN (value gin_trgm_ops);
