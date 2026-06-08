ALTER TABLE "relation_versions" ADD COLUMN "context_last_to_entity_id" uuid;--> statement-breakpoint
ALTER TABLE "value_versions" ADD COLUMN "context_last_to_entity_id" uuid;--> statement-breakpoint
CREATE INDEX "relation_versions_context_root_idx" ON "relation_versions" USING btree ("context_root_id","space_id","valid_from_key") WHERE "relation_versions"."context_root_id" IS NOT NULL;--> statement-breakpoint
CREATE INDEX "value_versions_context_root_idx" ON "value_versions" USING btree ("context_root_id","space_id","valid_from_key") WHERE "value_versions"."context_root_id" IS NOT NULL;