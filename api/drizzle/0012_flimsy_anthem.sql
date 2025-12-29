ALTER TABLE "user_votes" DROP CONSTRAINT "user_votes_user_entity_object_type_space_unique";--> statement-breakpoint
DROP INDEX "idx_user_votes_user_entity_object_type_space";--> statement-breakpoint
ALTER TABLE "user_votes" ADD COLUMN "user_id" uuid NOT NULL;--> statement-breakpoint
CREATE INDEX "idx_user_votes_user_entity_object_type_space" ON "user_votes" USING btree ("user_id","object_id","object_type","space_id");--> statement-breakpoint
ALTER TABLE "user_votes" DROP COLUMN "user_address";--> statement-breakpoint
ALTER TABLE "user_votes" ADD CONSTRAINT "user_votes_user_entity_object_type_space_unique" UNIQUE("user_id","object_id","object_type","space_id");