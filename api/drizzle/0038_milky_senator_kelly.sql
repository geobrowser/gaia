ALTER TABLE "proposal_tally_queue" DROP CONSTRAINT "proposal_tally_queue_proposal_id_proposals_id_fk";
--> statement-breakpoint
ALTER TABLE "edit_versions" ADD COLUMN "name" text;--> statement-breakpoint
ALTER TABLE "proposal_tally_queue" ADD CONSTRAINT "proposal_tally_queue_proposal_id_proposals_id_fk" FOREIGN KEY ("proposal_id") REFERENCES "public"."proposals"("id") ON DELETE cascade ON UPDATE no action;