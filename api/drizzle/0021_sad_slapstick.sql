-- Removed: enum recreation for properties.type - the properties table is dropped in migration 0023
-- Original lines 1-4 removed because they fail when existing data contains 'Relation' type
ALTER TABLE "values" ADD COLUMN "integer" bigint;--> statement-breakpoint
ALTER TABLE "values" ADD COLUMN "float" double precision;--> statement-breakpoint
ALTER TABLE "values" ADD COLUMN "bytes" "bytea";--> statement-breakpoint
ALTER TABLE "values" ADD COLUMN "date" text;--> statement-breakpoint
ALTER TABLE "values" ADD COLUMN "datetime" text;--> statement-breakpoint
ALTER TABLE "values" ADD COLUMN "schedule" jsonb;--> statement-breakpoint
ALTER TABLE "values" ADD COLUMN "embedding" jsonb;--> statement-breakpoint
CREATE INDEX "values_integer_idx" ON "values" USING btree ("integer");--> statement-breakpoint
CREATE INDEX "values_float_idx" ON "values" USING btree ("float");--> statement-breakpoint
CREATE INDEX "values_date_idx" ON "values" USING btree ("date");--> statement-breakpoint
CREATE INDEX "values_datetime_idx" ON "values" USING btree ("datetime");