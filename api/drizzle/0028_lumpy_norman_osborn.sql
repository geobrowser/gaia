ALTER TABLE "value_versions" ADD COLUMN "time_utc" time;--> statement-breakpoint
ALTER TABLE "value_versions" ADD COLUMN "datetime_utc" timestamp with time zone;--> statement-breakpoint
ALTER TABLE "values" ADD COLUMN "time_utc" time;--> statement-breakpoint
ALTER TABLE "values" ADD COLUMN "datetime_utc" timestamp with time zone;--> statement-breakpoint
CREATE INDEX "values_time_utc_idx" ON "values" USING btree ("time_utc");--> statement-breakpoint
CREATE INDEX "values_datetime_utc_idx" ON "values" USING btree ("datetime_utc");