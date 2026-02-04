DROP INDEX "proposals_end_time_idx";--> statement-breakpoint
CREATE INDEX "proposals_space_end_time_idx" ON "proposals" USING btree ("space_id","end_time");--> statement-breakpoint
CREATE INDEX "proposals_space_start_time_idx" ON "proposals" USING btree ("space_id","start_time");