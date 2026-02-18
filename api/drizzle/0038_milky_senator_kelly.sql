-- Hand-edited: removed Drizzle-generated FK drop/recreate for proposal_tally_queue.
-- Drizzle doesn't model DEFERRABLE constraints (see drizzle-orm#1429), so it
-- tried to reconcile the FK that migration 0037 made DEFERRABLE INITIALLY IMMEDIATE.
-- Letting Drizzle recreate it would silently drop the DEFERRABLE property, breaking
-- SET CONSTRAINTS ALL DEFERRED in the kg-indexer block transaction.
ALTER TABLE "edit_versions" ADD COLUMN "name" text;
