//! Sends Debate agenda listings to the analytics warehouse (GEO-3556, migration 0119), so "what
//! share of debates were on an agenda claim" is one warehouse query. Run by `agenda_listings_refresh`
//! right after each refresh, so it reads the set the refresh just wrote.
//!
//! Each run sends only what changed: listings new since the last run, and listings sent before that
//! the refresh has since deleted, marked `removed_at`, so an agenda edit never erases a past listing
//! from the warehouse. `trending.take_agenda_listing_changes()` returns those changes and records
//! them as sent; the transaction it runs in commits only after ClickHouse accepts the rows, so a
//! refused send is offered again by the next run. The first run sends every listing.
//!
//! The rows go to `analytics.gaia_agenda_listings` (analytics migration 124), a
//! `ReplacingMergeTree(exported_at)`, as a ClickHouse user granted INSERT on that table alone. Ids are
//! sent as the graph writes them elsewhere in the warehouse: 32 lowercase hex characters, no dashes.

use std::time::Duration;

use sqlx::PgPool;

use crate::clickhouse;
use crate::error::IndexerError;

pub const TABLE: &str = "analytics.gaia_agenda_listings";

/// One JSON object per change, keyed by the warehouse table's columns. `exported_at` is the run's
/// time, which is also a removal's `removed_at`, so the removal replaces the copy sent before.
const CHANGES_SQL: &str = r#"
    SELECT jsonb_build_object(
        'agenda_id', replace(c.agenda_id::text, '-', ''),
        'space_id', replace(c.space_id::text, '-', ''),
        'entity_id', replace(c.entity_id::text, '-', ''),
        'source', c.source,
        'listed_on', c.listed_on,
        'written_at', c.written_at,
        'removed_at', c.removed_at,
        'exported_at', now()
    )::text,
    c.removed_at IS NOT NULL
    FROM trending.take_agenda_listing_changes(now()) c
"#;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Sent {
    pub added: usize,
    pub removed: usize,
}

/// Sends what changed since the last run. On an error nothing is recorded as sent.
pub async fn export(pool: &PgPool, timeout: Duration) -> Result<Sent, IndexerError> {
    let mut tx = pool.begin().await?;
    let changes: Vec<(String, bool)> = sqlx::query_as(CHANGES_SQL).fetch_all(&mut *tx).await?;
    let removed = changes.iter().filter(|(_, removed)| *removed).count();
    let rows: Vec<String> = changes.into_iter().map(|(row, _)| row).collect();
    if !rows.is_empty() {
        // Dropping `tx` on an error rolls it back.
        clickhouse::insert_json_each_row(TABLE, &rows, timeout).await?;
    }
    tx.commit().await?;
    Ok(Sent {
        added: rows.len() - removed,
        removed,
    })
}
