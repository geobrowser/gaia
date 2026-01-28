ALTER TABLE "value_versions" ALTER COLUMN "time_utc" SET DATA TYPE time with time zone;--> statement-breakpoint
ALTER TABLE "value_versions" ALTER COLUMN "time_utc" DROP EXPRESSION;--> statement-breakpoint
ALTER TABLE "value_versions" ALTER COLUMN "datetime_utc" DROP EXPRESSION;--> statement-breakpoint
ALTER TABLE "values" ALTER COLUMN "time_utc" SET DATA TYPE time with time zone;--> statement-breakpoint
ALTER TABLE "values" ALTER COLUMN "time_utc" DROP EXPRESSION;--> statement-breakpoint
ALTER TABLE "values" ALTER COLUMN "datetime_utc" DROP EXPRESSION;--> statement-breakpoint
ALTER TABLE "relation_versions" ADD COLUMN "context_root_id" uuid;--> statement-breakpoint
ALTER TABLE "relation_versions" ADD COLUMN "context_edge_type_id" uuid;--> statement-breakpoint
ALTER TABLE "value_versions" ADD COLUMN "context_root_id" uuid;--> statement-breakpoint
ALTER TABLE "value_versions" ADD COLUMN "context_edge_type_id" uuid;