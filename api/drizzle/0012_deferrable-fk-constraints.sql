-- Make FK constraints deferrable so they can be checked at commit time
-- This allows inserting votes before proposals within the same transaction

-- proposal_actions -> proposals
ALTER TABLE "proposal_actions" DROP CONSTRAINT "proposal_actions_proposal_id_proposals_id_fk";
ALTER TABLE "proposal_actions" ADD CONSTRAINT "proposal_actions_proposal_id_proposals_id_fk"
  FOREIGN KEY ("proposal_id") REFERENCES "public"."proposals"("id")
  ON DELETE no action ON UPDATE no action
  DEFERRABLE INITIALLY IMMEDIATE;

-- proposal_votes -> proposals
ALTER TABLE "proposal_votes" DROP CONSTRAINT "proposal_votes_proposal_id_proposals_id_fk";
ALTER TABLE "proposal_votes" ADD CONSTRAINT "proposal_votes_proposal_id_proposals_id_fk"
  FOREIGN KEY ("proposal_id") REFERENCES "public"."proposals"("id")
  ON DELETE no action ON UPDATE no action
  DEFERRABLE INITIALLY IMMEDIATE;

-- proposal_votes -> spaces
ALTER TABLE "proposal_votes" DROP CONSTRAINT "proposal_votes_space_id_spaces_id_fk";
ALTER TABLE "proposal_votes" ADD CONSTRAINT "proposal_votes_space_id_spaces_id_fk"
  FOREIGN KEY ("space_id") REFERENCES "public"."spaces"("id")
  ON DELETE no action ON UPDATE no action
  DEFERRABLE INITIALLY IMMEDIATE;

-- proposals -> spaces
ALTER TABLE "proposals" DROP CONSTRAINT "proposals_space_id_spaces_id_fk";
ALTER TABLE "proposals" ADD CONSTRAINT "proposals_space_id_spaces_id_fk"
  FOREIGN KEY ("space_id") REFERENCES "public"."spaces"("id")
  ON DELETE no action ON UPDATE no action
  DEFERRABLE INITIALLY IMMEDIATE;
