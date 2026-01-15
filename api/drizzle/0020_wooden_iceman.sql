-- Convert text to uuid, setting invalid values (old checksummed addresses) to NULL
ALTER TABLE "proposal_actions" ALTER COLUMN "target_id" SET DATA TYPE uuid
USING CASE
    WHEN target_id ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
    THEN target_id::uuid
    ELSE NULL
END;