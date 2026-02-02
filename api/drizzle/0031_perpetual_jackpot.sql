ALTER TABLE "value_versions" ADD COLUMN "rect" text;--> statement-breakpoint
ALTER TABLE "values" ADD COLUMN "rect" text;--> statement-breakpoint
CREATE INDEX "values_rect_idx" ON "values" USING btree ("rect");