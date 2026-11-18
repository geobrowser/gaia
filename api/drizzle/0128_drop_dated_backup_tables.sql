-- Drop three dated backup tables left behind by past column migrations. Each holds only a key and
-- the pre-migration value of one column (edit_versions.created_at, proposals.created_at,
-- spaces.address), taken as a rollback aid before a migration that has long since completed. They
-- are not in schema.ts, nothing references them (no views or foreign keys), and they are ~3.5
-- months stale. PostGraphile auto-generates query fields over every public table, so while they
-- exist the indexer's public GraphQL schema exposes them as queryable connections. Dropping them
-- removes both the stale data and that exposure; the schema rebuilds when the api pods restart.
DROP TABLE IF EXISTS "edit_versions_created_at_backup_20260731";
--> statement-breakpoint
DROP TABLE IF EXISTS "proposals_created_at_backup_20260804";
--> statement-breakpoint
DROP TABLE IF EXISTS "spaces_address_backup_20260730";
