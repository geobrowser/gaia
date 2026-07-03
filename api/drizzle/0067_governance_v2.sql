CREATE TABLE "proposal_versions" (
	"proposal_id" uuid NOT NULL,
	"proposal_version" integer NOT NULL,
	"voting_mode" "votingMode" NOT NULL,
	"start_time" bigint NOT NULL,
	"end_time" bigint NOT NULL,
	"quorum" bigint NOT NULL,
	"threshold" bigint NOT NULL,
	"partial_percentage_support_threshold" bigint NOT NULL,
	"universal_percentage_support_threshold" bigint NOT NULL,
	"flat_support_threshold" bigint NOT NULL,
	"execute_by" bigint,
	"name" text,
	"yes_count" bigint DEFAULT 0 NOT NULL,
	"no_count" bigint DEFAULT 0 NOT NULL,
	"abstain_count" bigint DEFAULT 0 NOT NULL,
	"version_created_at" text NOT NULL,
	"version_created_at_block" text NOT NULL,
	CONSTRAINT "proposal_versions_proposal_id_proposal_version_pk" PRIMARY KEY("proposal_id","proposal_version"),
	CONSTRAINT "proposal_versions_idempotency_key" UNIQUE("proposal_id","version_created_at_block")
);
--> statement-breakpoint
CREATE TABLE "space_voting_settings" (
	"space_id" uuid PRIMARY KEY NOT NULL,
	"partial_percentage_support_threshold" bigint NOT NULL,
	"universal_percentage_support_threshold" bigint NOT NULL,
	"flat_support_threshold" bigint NOT NULL,
	"quorum" bigint NOT NULL,
	"duration" bigint NOT NULL,
	"disable_fast_path_access_for_new_members" boolean NOT NULL,
	"execution_grace_period" bigint NOT NULL,
	"updated_at" text NOT NULL,
	"updated_at_block" text NOT NULL
);
--> statement-breakpoint
ALTER TABLE "proposal_actions" DROP CONSTRAINT "proposal_actions_proposal_id_proposals_id_fk";
--> statement-breakpoint
ALTER TABLE "proposal_votes" DROP CONSTRAINT "proposal_votes_proposal_id_proposals_id_fk";
--> statement-breakpoint
DROP INDEX "proposals_space_end_time_idx";--> statement-breakpoint
DROP INDEX "proposals_space_start_time_idx";--> statement-breakpoint
ALTER TABLE "proposal_votes" DROP CONSTRAINT "proposal_votes_proposal_id_voter_id_pk";--> statement-breakpoint
ALTER TABLE "proposal_actions" DROP COLUMN "id";--> statement-breakpoint
ALTER TABLE "proposal_actions" ADD COLUMN "proposal_version" integer NOT NULL;--> statement-breakpoint
ALTER TABLE "proposal_actions" ADD COLUMN "index" integer NOT NULL;--> statement-breakpoint
ALTER TABLE "proposal_actions" ADD COLUMN "partial_percentage_support_threshold" bigint;--> statement-breakpoint
ALTER TABLE "proposal_actions" ADD COLUMN "universal_percentage_support_threshold" bigint;--> statement-breakpoint
ALTER TABLE "proposal_actions" ADD COLUMN "flat_support_threshold" bigint;--> statement-breakpoint
ALTER TABLE "proposal_actions" ADD COLUMN "disable_fast_path_access_for_new_members" boolean;--> statement-breakpoint
ALTER TABLE "proposal_actions" ADD COLUMN "execution_grace_period" bigint;--> statement-breakpoint
ALTER TABLE "proposal_votes" ADD COLUMN "proposal_version" integer NOT NULL;--> statement-breakpoint
ALTER TABLE "proposals" ADD COLUMN "current_version" integer DEFAULT 1 NOT NULL;--> statement-breakpoint
ALTER TABLE "proposal_actions" ADD CONSTRAINT "proposal_actions_proposal_id_proposal_version_index_pk" PRIMARY KEY("proposal_id","proposal_version","index");--> statement-breakpoint
ALTER TABLE "proposal_votes" ADD CONSTRAINT "proposal_votes_proposal_id_proposal_version_voter_id_pk" PRIMARY KEY("proposal_id","proposal_version","voter_id");--> statement-breakpoint
ALTER TABLE "proposal_versions" ADD CONSTRAINT "proposal_versions_proposal_id_proposals_id_fk" FOREIGN KEY ("proposal_id") REFERENCES "public"."proposals"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "space_voting_settings" ADD CONSTRAINT "space_voting_settings_space_id_spaces_id_fk" FOREIGN KEY ("space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "proposal_versions_proposal_version_desc_idx" ON "proposal_versions" USING btree ("proposal_id","proposal_version" DESC NULLS LAST);--> statement-breakpoint
CREATE INDEX "proposal_versions_end_time_idx" ON "proposal_versions" USING btree ("end_time");--> statement-breakpoint
CREATE INDEX "proposal_versions_start_time_idx" ON "proposal_versions" USING btree ("start_time");--> statement-breakpoint
ALTER TABLE "proposal_actions" ADD CONSTRAINT "proposal_actions_version_fk" FOREIGN KEY ("proposal_id","proposal_version") REFERENCES "public"."proposal_versions"("proposal_id","proposal_version") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "proposal_votes" ADD CONSTRAINT "proposal_votes_version_fk" FOREIGN KEY ("proposal_id","proposal_version") REFERENCES "public"."proposal_versions"("proposal_id","proposal_version") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "proposals" DROP COLUMN "voting_mode";--> statement-breakpoint
ALTER TABLE "proposals" DROP COLUMN "start_time";--> statement-breakpoint
ALTER TABLE "proposals" DROP COLUMN "end_time";--> statement-breakpoint
ALTER TABLE "proposals" DROP COLUMN "quorum";--> statement-breakpoint
ALTER TABLE "proposals" DROP COLUMN "threshold";--> statement-breakpoint
ALTER TABLE "proposals" DROP COLUMN "name";--> statement-breakpoint
ALTER TABLE "proposals" DROP COLUMN "yes_count";--> statement-breakpoint
ALTER TABLE "proposals" DROP COLUMN "no_count";--> statement-breakpoint
ALTER TABLE "proposals" DROP COLUMN "abstain_count";--> statement-breakpoint
CREATE VIEW "public"."proposals_current" AS (select "proposals"."id", "proposals"."space_id", "proposals"."proposed_by", "proposals"."created_at", "proposals"."created_at_block", "proposals"."current_version", "proposals"."executed_at", "proposal_versions"."proposal_id", "proposal_versions"."proposal_version", "proposal_versions"."voting_mode", "proposal_versions"."start_time", "proposal_versions"."end_time", "proposal_versions"."quorum", "proposal_versions"."threshold", "proposal_versions"."partial_percentage_support_threshold", "proposal_versions"."universal_percentage_support_threshold", "proposal_versions"."flat_support_threshold", "proposal_versions"."execute_by", "proposal_versions"."name", "proposal_versions"."yes_count", "proposal_versions"."no_count", "proposal_versions"."abstain_count", "proposal_versions"."version_created_at", "proposal_versions"."version_created_at_block" from "proposals" inner join "proposal_versions" on "proposal_versions"."proposal_id" = "proposals"."id" AND "proposal_versions"."proposal_version" = "proposals"."current_version");--> statement-breakpoint
CREATE VIEW "public"."space_editor_counts" AS (SELECT space_id, COUNT(*)::bigint AS total_editors FROM editors GROUP BY space_id);--> statement-breakpoint
-- Hand-edited: added DEFERRABLE INITIALLY IMMEDIATE so SET CONSTRAINTS ALL DEFERRED
-- in the kg-indexer block transaction can defer these FKs until commit time.
-- Same pattern as migrations 0012, 0037, 0048. Not expressible in schema.ts, so
-- drizzle-kit regenerates these FKs as non-deferrable; re-apply the property here.
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