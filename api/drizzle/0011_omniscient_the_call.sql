CREATE TYPE "public"."proposalActionType" AS ENUM('AddMember', 'RemoveMember', 'AddEditor', 'RemoveEditor', 'UnflagEditor', 'Publish', 'Flag', 'Unflag', 'UpdateVotingSettings', 'Unknown');--> statement-breakpoint
CREATE TYPE "public"."voteOption" AS ENUM('Yes', 'No', 'Abstain');--> statement-breakpoint
CREATE TYPE "public"."votingMode" AS ENUM('Fast', 'Slow');--> statement-breakpoint
CREATE TABLE "proposal_actions" (
	"id" uuid PRIMARY KEY NOT NULL,
	"proposal_id" uuid NOT NULL,
	"action_type" "proposalActionType" NOT NULL,
	"target_address" text,
	"content_uri" text,
	"metadata" "bytea",
	"content_id" "bytea",
	"quorum" bigint,
	"fast_threshold" bigint,
	"slow_threshold" bigint,
	"duration" bigint
);
--> statement-breakpoint
CREATE TABLE "proposal_votes" (
	"proposal_id" uuid NOT NULL,
	"voter_id" uuid NOT NULL,
	"space_id" uuid NOT NULL,
	"vote" "voteOption" NOT NULL,
	"created_at" text NOT NULL,
	"created_at_block" text NOT NULL,
	CONSTRAINT "proposal_votes_proposal_id_voter_id_pk" PRIMARY KEY("proposal_id","voter_id")
);
--> statement-breakpoint
CREATE TABLE "proposals" (
	"id" uuid PRIMARY KEY NOT NULL,
	"space_id" uuid NOT NULL,
	"proposed_by" uuid NOT NULL,
	"voting_mode" "votingMode" NOT NULL,
	"start_time" bigint NOT NULL,
	"end_time" bigint NOT NULL,
	"quorum" bigint NOT NULL,
	"threshold" bigint NOT NULL,
	"executed_at" bigint,
	"created_at" text NOT NULL,
	"created_at_block" text NOT NULL
);
--> statement-breakpoint
ALTER TABLE "proposal_actions" ADD CONSTRAINT "proposal_actions_proposal_id_proposals_id_fk" FOREIGN KEY ("proposal_id") REFERENCES "public"."proposals"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "proposal_votes" ADD CONSTRAINT "proposal_votes_proposal_id_proposals_id_fk" FOREIGN KEY ("proposal_id") REFERENCES "public"."proposals"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "proposal_votes" ADD CONSTRAINT "proposal_votes_space_id_spaces_id_fk" FOREIGN KEY ("space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "proposals" ADD CONSTRAINT "proposals_space_id_spaces_id_fk" FOREIGN KEY ("space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "proposal_actions_proposal_id_idx" ON "proposal_actions" USING btree ("proposal_id");--> statement-breakpoint
CREATE INDEX "proposal_actions_action_type_idx" ON "proposal_actions" USING btree ("action_type");--> statement-breakpoint
CREATE INDEX "proposal_votes_space_id_idx" ON "proposal_votes" USING btree ("space_id");--> statement-breakpoint
CREATE INDEX "proposal_votes_voter_id_idx" ON "proposal_votes" USING btree ("voter_id");--> statement-breakpoint
CREATE INDEX "proposal_votes_vote_idx" ON "proposal_votes" USING btree ("vote");--> statement-breakpoint
CREATE INDEX "proposals_space_id_idx" ON "proposals" USING btree ("space_id");--> statement-breakpoint
CREATE INDEX "proposals_proposed_by_idx" ON "proposals" USING btree ("proposed_by");--> statement-breakpoint
CREATE INDEX "proposals_created_at_idx" ON "proposals" USING btree ("created_at");--> statement-breakpoint
CREATE INDEX "proposals_end_time_idx" ON "proposals" USING btree ("end_time");