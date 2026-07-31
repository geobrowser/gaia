-- Large envs: build CONCURRENTLY first (then the IF NOT EXISTS below no-ops):
--   SET lock_timeout=0; SET statement_timeout=0;
--   CREATE INDEX CONCURRENTLY IF NOT EXISTS "relations_to_entity_type_idx" ON "relations" USING btree ("to_entity_id","type_id");
CREATE INDEX IF NOT EXISTS "relations_to_entity_type_idx" ON "relations" USING btree ("to_entity_id","type_id");
