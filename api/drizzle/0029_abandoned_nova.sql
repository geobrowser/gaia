ALTER TABLE "relation_versions" ADD COLUMN "context_root_id" uuid;--> statement-breakpoint
ALTER TABLE "relation_versions" ADD COLUMN "context_edge_type_id" uuid;--> statement-breakpoint
ALTER TABLE "value_versions" ADD COLUMN "context_root_id" uuid;--> statement-breakpoint
ALTER TABLE "value_versions" ADD COLUMN "context_edge_type_id" uuid;