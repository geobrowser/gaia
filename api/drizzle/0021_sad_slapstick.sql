ALTER TABLE "properties" ALTER COLUMN "type" SET DATA TYPE text;--> statement-breakpoint
DROP TYPE "public"."dataTypes";--> statement-breakpoint
CREATE TYPE "public"."dataTypes" AS ENUM('Bool', 'Int64', 'Float64', 'Decimal', 'Text', 'Bytes', 'Date', 'Time', 'Datetime', 'Schedule', 'Point', 'Embedding');--> statement-breakpoint
ALTER TABLE "properties" ALTER COLUMN "type" SET DATA TYPE "public"."dataTypes" USING "type"::"public"."dataTypes";--> statement-breakpoint
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