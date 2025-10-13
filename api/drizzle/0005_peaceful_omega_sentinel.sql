CREATE TABLE "votes" (
	"id" uuid PRIMARY KEY NOT NULL,
	"onchain_proposal_id" text NOT NULL,
	"voter_address" varchar(42) NOT NULL,
	"vote_option" smallint NOT NULL,
	"plugin_address" varchar(42) NOT NULL,
	"space_id" uuid NOT NULL,
	"proposal_id" uuid,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "votes_onchain_proposal_voter_unique" UNIQUE("onchain_proposal_id","voter_address")
);
--> statement-breakpoint
ALTER TABLE "votes" ADD CONSTRAINT "votes_space_id_spaces_id_fk" FOREIGN KEY ("space_id") REFERENCES "public"."spaces"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "votes" ADD CONSTRAINT "votes_proposal_id_proposals_id_fk" FOREIGN KEY ("proposal_id") REFERENCES "public"."proposals"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "votes_space_id_idx" ON "votes" USING btree ("space_id");--> statement-breakpoint
CREATE INDEX "votes_proposal_id_idx" ON "votes" USING btree ("proposal_id");--> statement-breakpoint
CREATE INDEX "votes_voter_idx" ON "votes" USING btree ("voter_address");--> statement-breakpoint
CREATE INDEX "votes_plugin_address_idx" ON "votes" USING btree ("plugin_address");--> statement-breakpoint
CREATE INDEX "votes_onchain_proposal_idx" ON "votes" USING btree ("onchain_proposal_id");--> statement-breakpoint
CREATE INDEX "votes_space_proposal_idx" ON "votes" USING btree ("space_id","proposal_id");