ALTER TABLE "user_votes" DROP CONSTRAINT "user_votes_user_entity_object_type_space_unique";--> statement-breakpoint
DROP INDEX "idx_user_votes_user_entity_object_type_space";--> statement-breakpoint
ALTER TABLE "raw_actions" ADD COLUMN "user_id" uuid NOT NULL;--> statement-breakpoint
ALTER TABLE "raw_actions" DROP COLUMN "sender";--> statement-breakpoint
ALTER TABLE "user_votes" DROP COLUMN "user_id";