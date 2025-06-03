-- Enable pg_trgm extension for fuzzy search
CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- Drop the basic text index created by the schema
DROP INDEX IF EXISTS "values_text_idx";

-- Create GIN index for trigram fuzzy search
CREATE INDEX "values_text_trgm_idx" ON "values" USING gin ("value" gin_trgm_ops);

-- Create GIN index for full-text search
CREATE INDEX "values_fts_idx" ON "values" USING gin (to_tsvector('english', "value"));

-- Add comment for documentation
COMMENT ON INDEX "values_text_trgm_idx" IS 'GIN index for trigram-based fuzzy search on entity values';
COMMENT ON INDEX "values_fts_idx" IS 'GIN index for full-text search on entity values';