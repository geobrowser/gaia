ALTER TABLE "value_versions" RENAME COLUMN "number" TO "decimal";--> statement-breakpoint
ALTER TABLE "value_versions" RENAME COLUMN "string" TO "text";--> statement-breakpoint
ALTER TABLE "values" RENAME COLUMN "number" TO "decimal";--> statement-breakpoint
ALTER TABLE "values" RENAME COLUMN "string" TO "text";--> statement-breakpoint
DROP INDEX "values_number_idx";--> statement-breakpoint
DROP INDEX "values_text_idx";--> statement-breakpoint
CREATE INDEX "values_decimal_idx" ON "values" USING btree ("decimal");--> statement-breakpoint
CREATE INDEX "values_text_idx" ON "values" USING btree ("text") WHERE length("values"."text") <= 2000;