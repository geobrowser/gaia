CREATE TABLE "space_voting_settings" (
	"space_id" uuid PRIMARY KEY NOT NULL,
	"partial_percentage_support_threshold" bigint NOT NULL,
	"universal_percentage_support_threshold" bigint NOT NULL,
	"flat_support_threshold" bigint NOT NULL,
	"quorum" bigint NOT NULL,
	"duration" bigint NOT NULL,
	"disable_fast_path_access_for_new_members" boolean NOT NULL,
	"execution_grace_period" bigint NOT NULL,
	"total_editors" bigint DEFAULT 0 NOT NULL,
	"updated_at" text NOT NULL,
	"updated_at_block" text NOT NULL
);
--> statement-breakpoint
ALTER TABLE "proposal_actions" ADD COLUMN "partial_percentage_support_threshold" bigint;--> statement-breakpoint
ALTER TABLE "proposal_actions" ADD COLUMN "universal_percentage_support_threshold" bigint;--> statement-breakpoint
ALTER TABLE "proposal_actions" ADD COLUMN "flat_support_threshold" bigint;--> statement-breakpoint
ALTER TABLE "proposal_actions" ADD COLUMN "disable_fast_path_access_for_new_members" boolean;--> statement-breakpoint
ALTER TABLE "proposal_actions" ADD COLUMN "execution_grace_period" bigint;--> statement-breakpoint
ALTER TABLE "proposals" ADD COLUMN "proposal_version" integer DEFAULT 1 NOT NULL;--> statement-breakpoint
ALTER TABLE "proposals" ADD COLUMN "partial_percentage_support_threshold" bigint;--> statement-breakpoint
ALTER TABLE "proposals" ADD COLUMN "universal_percentage_support_threshold" bigint;--> statement-breakpoint
ALTER TABLE "proposals" ADD COLUMN "flat_support_threshold" bigint;--> statement-breakpoint
ALTER TABLE "proposals" ADD COLUMN "execute_by" bigint;--> statement-breakpoint
ALTER TABLE "space_voting_settings" ADD CONSTRAINT "space_voting_settings_space_id_spaces_id_fk" FOREIGN KEY ("space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action;