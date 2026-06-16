-- For large tables, build the indexes manually with CONCURRENTLY before this runs (then the
-- IF NOT EXISTS below no-op):
--   CREATE INDEX CONCURRENTLY IF NOT EXISTS "relation_versions_context_root_idx" ON "relation_versions" USING btree ("context_root_id","space_id","valid_from_key") WHERE "context_root_id" IS NOT NULL;
--   CREATE INDEX CONCURRENTLY IF NOT EXISTS "value_versions_context_root_idx" ON "value_versions" USING btree ("context_root_id","space_id","valid_from_key") WHERE "context_root_id" IS NOT NULL;
ALTER TABLE "relation_versions" ADD COLUMN IF NOT EXISTS "context_last_to_entity_id" uuid;--> statement-breakpoint
ALTER TABLE "value_versions" ADD COLUMN IF NOT EXISTS "context_last_to_entity_id" uuid;--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "relation_versions_context_root_idx" ON "relation_versions" USING btree ("context_root_id","space_id","valid_from_key") WHERE "relation_versions"."context_root_id" IS NOT NULL;--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "value_versions_context_root_idx" ON "value_versions" USING btree ("context_root_id","space_id","valid_from_key") WHERE "value_versions"."context_root_id" IS NOT NULL;
