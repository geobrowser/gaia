CREATE TABLE IF NOT EXISTS "ranks"."editors" (
	"member_space_id" uuid NOT NULL,
	"space_id" uuid NOT NULL,
	CONSTRAINT "editors_member_space_id_space_id_pk" PRIMARY KEY("member_space_id","space_id")
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "ranks"."members" (
	"member_space_id" uuid NOT NULL,
	"space_id" uuid NOT NULL,
	CONSTRAINT "members_member_space_id_space_id_pk" PRIMARY KEY("member_space_id","space_id")
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "ranks_editors_space_id_idx" ON "ranks"."editors" USING btree ("space_id");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "ranks_members_space_id_idx" ON "ranks"."members" USING btree ("space_id");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "rankings_space_id_idx" ON "ranks"."rankings" USING btree ("space_id");--> statement-breakpoint
-- Seed the ranking-indexer's membership view from the kg-indexer-maintained
-- public tables. Eligibility reads the public tables until this migration runs,
-- so the copy makes the switch-over a semantic no-op; only events consumed
-- after deploy diverge.
INSERT INTO "ranks"."members" ("member_space_id", "space_id")
SELECT "member_space_id", "space_id" FROM "public"."members"
ON CONFLICT DO NOTHING;--> statement-breakpoint
INSERT INTO "ranks"."editors" ("member_space_id", "space_id")
SELECT "member_space_id", "space_id" FROM "public"."editors"
ON CONFLICT DO NOTHING;
