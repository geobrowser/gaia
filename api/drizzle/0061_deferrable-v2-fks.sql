-- Hand-edited: added DEFERRABLE INITIALLY IMMEDIATE so SET CONSTRAINTS ALL DEFERRED
-- in the kg-indexer block transaction can defer these FKs until commit time.
-- Same pattern as migrations 0012, 0037, 0048.

ALTER TABLE "proposal_versions" DROP CONSTRAINT "proposal_versions_proposal_id_proposals_id_fk";--> statement-breakpoint
ALTER TABLE "proposal_versions" ADD CONSTRAINT "proposal_versions_proposal_id_proposals_id_fk"
  FOREIGN KEY ("proposal_id") REFERENCES "public"."proposals"("id")
  ON DELETE no action ON UPDATE no action
  DEFERRABLE INITIALLY IMMEDIATE;--> statement-breakpoint

ALTER TABLE "proposal_actions" DROP CONSTRAINT "proposal_actions_version_fk";--> statement-breakpoint
ALTER TABLE "proposal_actions" ADD CONSTRAINT "proposal_actions_version_fk"
  FOREIGN KEY ("proposal_id","proposal_version") REFERENCES "public"."proposal_versions"("proposal_id","proposal_version")
  ON DELETE no action ON UPDATE no action
  DEFERRABLE INITIALLY IMMEDIATE;--> statement-breakpoint

ALTER TABLE "proposal_votes" DROP CONSTRAINT "proposal_votes_version_fk";--> statement-breakpoint
ALTER TABLE "proposal_votes" ADD CONSTRAINT "proposal_votes_version_fk"
  FOREIGN KEY ("proposal_id","proposal_version") REFERENCES "public"."proposal_versions"("proposal_id","proposal_version")
  ON DELETE no action ON UPDATE no action
  DEFERRABLE INITIALLY IMMEDIATE;--> statement-breakpoint

ALTER TABLE "space_voting_settings" DROP CONSTRAINT "space_voting_settings_space_id_spaces_id_fk";--> statement-breakpoint
ALTER TABLE "space_voting_settings" ADD CONSTRAINT "space_voting_settings_space_id_spaces_id_fk"
  FOREIGN KEY ("space_id") REFERENCES "public"."spaces"("id")
  ON DELETE no action ON UPDATE no action
  DEFERRABLE INITIALLY IMMEDIATE;
