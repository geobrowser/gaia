-- Large envs: build CONCURRENTLY first (then the IF NOT EXISTS below no-ops):
--   SET lock_timeout=0; SET statement_timeout=0;
--   CREATE INDEX CONCURRENTLY IF NOT EXISTS "relation_versions_blocks_to_entity_idx" ON "relation_versions" USING btree ("to_entity_id","space_id","valid_from_key") WHERE "type_id" = 'beaba5cb-a677-41a8-b353-77030613fc70'::uuid;
CREATE INDEX IF NOT EXISTS "relation_versions_blocks_to_entity_idx" ON "relation_versions" USING btree ("to_entity_id","space_id","valid_from_key") WHERE "relation_versions"."type_id" = 'beaba5cb-a677-41a8-b353-77030613fc70'::uuid;
