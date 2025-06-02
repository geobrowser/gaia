ALTER TABLE "ipfs_cache" ALTER COLUMN "space" SET DATA TYPE uuid USING space::uuid;
