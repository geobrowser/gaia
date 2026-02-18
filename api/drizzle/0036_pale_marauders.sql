CREATE TABLE "proposal_tally_queue" (
	"proposal_id" uuid PRIMARY KEY NOT NULL,
	"queued_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
ALTER TABLE "proposals" ADD COLUMN "yes_count" bigint DEFAULT 0 NOT NULL;--> statement-breakpoint
ALTER TABLE "proposals" ADD COLUMN "no_count" bigint DEFAULT 0 NOT NULL;--> statement-breakpoint
ALTER TABLE "proposals" ADD COLUMN "abstain_count" bigint DEFAULT 0 NOT NULL;--> statement-breakpoint
ALTER TABLE "proposal_tally_queue" ADD CONSTRAINT "proposal_tally_queue_proposal_id_proposals_id_fk" FOREIGN KEY ("proposal_id") REFERENCES "public"."proposals"("id") ON DELETE CASCADE ON UPDATE no action;--> statement-breakpoint
-- Backfill existing proposal vote tallies from proposal_votes table
UPDATE proposals p
SET
    yes_count = COALESCE(vc.yes_count, 0),
    no_count = COALESCE(vc.no_count, 0),
    abstain_count = COALESCE(vc.abstain_count, 0)
FROM (
    SELECT
        proposal_id,
        COUNT(*) FILTER (WHERE vote = 'Yes') as yes_count,
        COUNT(*) FILTER (WHERE vote = 'No') as no_count,
        COUNT(*) FILTER (WHERE vote = 'Abstain') as abstain_count
    FROM proposal_votes
    GROUP BY proposal_id
) vc
WHERE p.id = vc.proposal_id;