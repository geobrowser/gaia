ALTER TABLE "raw_actions" RENAME COLUMN "entity" TO "object_id";--> statement-breakpoint
ALTER TABLE "user_votes" RENAME COLUMN "entity_id" TO "object_id";--> statement-breakpoint
ALTER TABLE "votes_count" RENAME COLUMN "entity_id" TO "object_id";--> statement-breakpoint
ALTER TABLE "user_votes" DROP CONSTRAINT "user_votes_user_entity_space_unique";--> statement-breakpoint
ALTER TABLE "votes_count" DROP CONSTRAINT "votes_count_entity_space_unique";--> statement-breakpoint
DROP INDEX "idx_user_votes_user_entity_space";--> statement-breakpoint
DROP INDEX "idx_votes_count_entity_space";--> statement-breakpoint
ALTER TABLE "raw_actions" ADD COLUMN "object_type" smallint NOT NULL;--> statement-breakpoint
ALTER TABLE "user_votes" ADD COLUMN "object_type" smallint NOT NULL;--> statement-breakpoint
ALTER TABLE "votes_count" ADD COLUMN "object_type" smallint NOT NULL;--> statement-breakpoint
CREATE INDEX "idx_user_votes_user_entity_object_type_space" ON "user_votes" USING btree ("user_id","object_id","object_type","space_id");--> statement-breakpoint
CREATE INDEX "idx_votes_count_object_object_type_space" ON "votes_count" USING btree ("object_id","object_type","space_id");--> statement-breakpoint
ALTER TABLE "user_votes" ADD CONSTRAINT "user_votes_user_entity_object_type_space_unique" UNIQUE("user_id","object_id","object_type","space_id");--> statement-breakpoint
ALTER TABLE "votes_count" ADD CONSTRAINT "votes_count_object_object_type_space_unique" UNIQUE("object_id","object_type","space_id");