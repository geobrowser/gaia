# Plan: Add UTC-Normalized Time Columns

## Summary

Add `time_utc` and `datetime_utc` columns to store UTC-normalized versions of time/datetime values. Keep existing text columns for original values with offsets.

## Design Decisions

| Field | PostgreSQL Type | Rust Type | Notes |
|-------|-----------------|-----------|-------|
| `datetime_utc` | `TIMESTAMP WITH TIME ZONE` | `DateTime<Utc>` | Stores as UTC, auto-converts on retrieval |
| `time_utc` | `TIME` (without timezone) | `NaiveTime` | Plain time; PG docs discourage TIME WITH TIME ZONE |

**Error handling**: If parsing fails, log warning and leave UTC column NULL.

## Files to Modify

### 1. kg-indexer/Cargo.toml

Add `chrono` feature to sqlx:

```toml
sqlx = { version = "0.8", features = ["...", "chrono"] }
```

### 2. kg-indexer/src/models/values.rs

Add fields to `ValueOp` struct:

```rust
pub time_utc: Option<NaiveTime>,       // UTC-normalized time
pub datetime_utc: Option<DateTime<Utc>>, // UTC-normalized datetime
```

### 3. kg-indexer/src/handlers/edits.rs

Add parsing helpers:

- `parse_time_to_utc(time_str: &str) -> Option<NaiveTime>` - Parse RFC 3339 time, apply offset
- `parse_datetime_to_utc(datetime_str: &str) -> Option<DateTime<Utc>>` - Parse RFC 3339 datetime

Update `value_to_value_op` for `Grc20Value::Time` and `Grc20Value::Datetime` cases to populate both original and UTC fields.

### 4. kg-indexer/src/storage.rs

Update two functions:

- `insert_values` (line 74): Add `time_utc` and `datetime_utc` to INSERT query
- `insert_value_versions` (line 1047): Same changes

SQL column types: `$N::time[]` for time_utc, `$N::timestamptz[]` for datetime_utc

### 5. api/src/services/storage/schema.ts

Add columns to both tables:

**values table** (line 106):

```typescript
timeUtc: time("time_utc"),
datetimeUtc: timestamp("datetime_utc", { withTimezone: true, mode: "date" }),
```

**valueVersions table** (line 690):

```typescript
timeUtc: time("time_utc"),
datetimeUtc: timestamp("datetime_utc", { withTimezone: true, mode: "date" }),
```

Add indexes: `values_time_utc_idx`, `values_datetime_utc_idx`

### 6. Migration (new file)

Create `api/drizzle/XXXX_add_utc_time_columns.sql`:

```sql
ALTER TABLE "values" ADD COLUMN "time_utc" time;
ALTER TABLE "values" ADD COLUMN "datetime_utc" timestamp with time zone;
ALTER TABLE "value_versions" ADD COLUMN "time_utc" time;
ALTER TABLE "value_versions" ADD COLUMN "datetime_utc" timestamp with time zone;
CREATE INDEX "values_time_utc_idx" ON "values" ("time_utc");
CREATE INDEX "values_datetime_utc_idx" ON "values" ("datetime_utc");
```

## Implementation Order

1. Update `Cargo.toml` (add chrono feature)
2. Update `values.rs` (add struct fields)
3. Update `edits.rs` (add parsing logic)
4. Update `storage.rs` (add SQL columns/bindings)
5. Update `schema.ts` (add Drizzle columns)
6. Generate migration with `bun drizzle-kit generate`
7. Build and test kg-indexer: `cargo build && cargo test`
8. Apply migration to database

## Notes

- **Time wraparound**: "02:30:00+05:30" → "21:00:00" (previous day). Date info lost; acceptable for time-only field.
- **PostGraphile**: Will auto-expose `timeUtc` and `datetimeUtc` in GraphQL.
- **Existing data**: New columns will be NULL. Backfill can be done separately if needed.
- **grc-20 format**: Always includes offset (`Z` or `+HH:MM`), defaults to UTC when unspecified.
