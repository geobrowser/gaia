# Atlas Checkpoint Persistence Rollout Checklist

## Preconditions

- Migration `api/drizzle/0044_atlas-checkpoints.sql` has been applied in target environment.
- `ATLAS_CHECKPOINT_DATABASE_URL` is set.
- `ATLAS_INDEXER_ID` is set and non-empty.
- `ATLAS_RUNTIME_COMPATIBILITY_MARKER` and runtime `graph_state_version` are unchanged unless intentionally performing a fresh bootstrap.

## Pre-Deploy SQL Checks

```sql
SELECT to_regclass('public.atlas_checkpoints') AS table_ref;

SELECT indexer_id, schema_version, graph_state_version, runtime_compatibility_marker, root_space_id
FROM atlas_checkpoints
WHERE indexer_id = $1;
```

## Post-Deploy Checks

- Verify logs include `Checkpoint persisted` events for the expected `indexer_id`.
- Verify no repeated `Checkpoint persist failed; retrying` warnings.
- Verify no `Fail-open bound exceeded; pausing processing until checkpoint write recovers` errors.

```sql
SELECT indexer_id, block_number, updated_at
FROM atlas_checkpoints
WHERE indexer_id = $1;
```

Expected:
- row exists for current indexer
- `block_number` advances during normal ingest
- `updated_at` remains fresh

## Rollback Notes

- Current checkpoint backend is Postgres-only.
- Rolling back to a pre-Postgres Atlas binary can lose checkpoint continuity unless that binary can read `atlas_checkpoints`.
- Safe rollback options:
  1. Roll back to a binary that still supports Postgres checkpoints, or
  2. Disable checkpoint persistence and accept replay from configured start behavior.

## Incident Triage

If persistence outages occur:
- Atlas may continue up to `ATLAS_FAIL_OPEN_BOUND` uncheckpointed blocks.
- Atlas pauses when the bound is exceeded and resumes after successful checkpoint recovery.
- Investigate DB connectivity, credentials, and table permissions first.
