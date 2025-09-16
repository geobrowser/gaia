ALTER TABLE "raw_actions" ALTER COLUMN "space_pov" SET DATA TYPE uuid USING "space_pov"::uuid;
ALTER TABLE "user_votes" ALTER COLUMN "space_id" SET DATA TYPE uuid USING "space_id"::uuid;
ALTER TABLE "votes_count" ALTER COLUMN "space_id" SET DATA TYPE uuid USING "space_id"::uuid;
