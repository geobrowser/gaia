-- Make proposal_tally_queue FK deferrable so SET CONSTRAINTS ALL DEFERRED
-- works in the kg-indexer block transaction (consistent with migration 0012).
-- Without this, a PROPOSAL_VOTED event processed before its PROPOSAL_CREATED
-- in the same block causes an immediate FK violation on tally queue insert.

ALTER TABLE "proposal_tally_queue" DROP CONSTRAINT "proposal_tally_queue_proposal_id_proposals_id_fk";
ALTER TABLE "proposal_tally_queue" ADD CONSTRAINT "proposal_tally_queue_proposal_id_proposals_id_fk"
  FOREIGN KEY ("proposal_id") REFERENCES "public"."proposals"("id")
  ON DELETE CASCADE ON UPDATE no action
  DEFERRABLE INITIALLY IMMEDIATE;
