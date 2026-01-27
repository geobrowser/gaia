-- Set database timezone to UTC.
-- PostgreSQL stores timestamptz as UTC internally but displays in session timezone.
-- By setting the database default to UTC, all connections return UTC without
-- needing per-connection configuration. This ensures the *_utc columns return
-- normalized UTC values for consistent querying and sorting.
DO $$
BEGIN
  EXECUTE format('ALTER DATABASE %I SET timezone = %L', current_database(), 'UTC');
END $$;--> statement-breakpoint
ALTER TABLE "value_versions" ADD COLUMN "time_utc" time with time zone;--> statement-breakpoint
ALTER TABLE "value_versions" ADD COLUMN "datetime_utc" timestamp with time zone;--> statement-breakpoint
ALTER TABLE "values" ADD COLUMN "time_utc" time with time zone;--> statement-breakpoint
ALTER TABLE "values" ADD COLUMN "datetime_utc" timestamp with time zone;--> statement-breakpoint
CREATE INDEX "values_time_utc_idx" ON "values" USING btree ("time_utc");--> statement-breakpoint
CREATE INDEX "values_datetime_utc_idx" ON "values" USING btree ("datetime_utc");
