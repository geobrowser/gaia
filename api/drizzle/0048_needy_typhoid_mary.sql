-- Hand-edited: added DEFERRABLE INITIALLY IMMEDIATE so SET CONSTRAINTS ALL DEFERRED
-- in the kg-indexer block transaction can defer these until commit time.
-- Drizzle doesn't model DEFERRABLE (see drizzle-orm#1429).
ALTER TABLE "subspaces" ADD CONSTRAINT "subspaces_parent_space_id_spaces_id_fk" FOREIGN KEY ("parent_space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action DEFERRABLE INITIALLY IMMEDIATE;--> statement-breakpoint
ALTER TABLE "subspaces" ADD CONSTRAINT "subspaces_child_space_id_spaces_id_fk" FOREIGN KEY ("child_space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action DEFERRABLE INITIALLY IMMEDIATE;