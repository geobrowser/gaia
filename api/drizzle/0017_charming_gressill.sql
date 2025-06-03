CREATE INDEX "values_text_idx" ON "values" USING btree ("value");--> statement-breakpoint
CREATE INDEX "values_space_text_idx" ON "values" USING btree ("space_id","value");