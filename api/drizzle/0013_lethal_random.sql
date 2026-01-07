ALTER TABLE "ipfs_cache" RENAME COLUMN "json" TO "data";--> statement-breakpoint
ALTER TABLE "ipfs_cache" ALTER COLUMN "data" TYPE bytea USING NULL;