ALTER TABLE "value_versions" ADD COLUMN "time_utc" time GENERATED ALWAYS AS (("value_versions"."time"::timetz AT TIME ZONE 'UTC')::time) STORED;--> statement-breakpoint
ALTER TABLE "value_versions" ADD COLUMN "datetime_utc" timestamp with time zone GENERATED ALWAYS AS ("value_versions"."datetime"::timestamptz) STORED;--> statement-breakpoint
ALTER TABLE "values" ADD COLUMN "time_utc" time GENERATED ALWAYS AS (("values"."time"::timetz AT TIME ZONE 'UTC')::time) STORED;--> statement-breakpoint
ALTER TABLE "values" ADD COLUMN "datetime_utc" timestamp with time zone GENERATED ALWAYS AS ("values"."datetime"::timestamptz) STORED;--> statement-breakpoint
CREATE INDEX "values_time_utc_idx" ON "values" USING btree ("time_utc");--> statement-breakpoint
CREATE INDEX "values_datetime_utc_idx" ON "values" USING btree ("datetime_utc");