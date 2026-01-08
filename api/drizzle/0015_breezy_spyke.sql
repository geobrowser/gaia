ALTER TABLE "spaces" RENAME COLUMN "dao_address" TO "address";--> statement-breakpoint
ALTER TABLE "spaces" ALTER COLUMN "type" SET DATA TYPE text;--> statement-breakpoint
DROP TYPE "public"."spaceTypes";--> statement-breakpoint
CREATE TYPE "public"."spaceTypes" AS ENUM('DAO', 'Personal');--> statement-breakpoint
ALTER TABLE "spaces" ALTER COLUMN "type" SET DATA TYPE "public"."spaceTypes" USING "type"::"public"."spaceTypes";--> statement-breakpoint
ALTER TABLE "spaces" ADD COLUMN "topic_id" uuid;--> statement-breakpoint
ALTER TABLE "spaces" DROP COLUMN "space_address";--> statement-breakpoint
ALTER TABLE "spaces" DROP COLUMN "main_voting_address";--> statement-breakpoint
ALTER TABLE "spaces" DROP COLUMN "membership_address";--> statement-breakpoint
ALTER TABLE "spaces" DROP COLUMN "personal_address";