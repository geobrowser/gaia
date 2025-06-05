CREATE TYPE "public"."spaceTypes" AS ENUM('Personal', 'Public');--> statement-breakpoint
ALTER TABLE "spaces" ALTER COLUMN "dao_address" SET NOT NULL;--> statement-breakpoint
ALTER TABLE "spaces" ADD COLUMN "type" "spaceTypes" NOT NULL;--> statement-breakpoint
ALTER TABLE "spaces" ADD COLUMN "space_address" text NOT NULL;--> statement-breakpoint
ALTER TABLE "spaces" ADD COLUMN "main_voting_address" text;--> statement-breakpoint
ALTER TABLE "spaces" ADD COLUMN "membership_address" text;--> statement-breakpoint
ALTER TABLE "spaces" ADD COLUMN "personal_address" text;