//! "Not interested" from geo-chat into the topic-interest model (GEO-3088, migration 0105).
//!
//! Not interested on a claim (GEO-2862) is stored in geo-chat, off-chain and private:
//! `claim_not_interested (user_id, claim_entity_id, created_at)`, where geo-chat's
//! `users.profile_space_id` is the person's Geo personal space, the id gaia keys a person on. This
//! module reads the whole current set and hands it to
//! `personalization.replace_external_interest_signals`, which replaces every `not_interested` row
//! and recomputes each user whose set changed. A snapshot rather than events, so an undo is simply a
//! missing row and a missed run is repaired by the next.
//!
//! Ids are normalised the way geo-chat normalises space ids (`common::normalize_space_id`): trimmed,
//! lower-cased, an optional `0x` dropped, a 64-hex value zero-padded from 32 cut back, dashes
//! optional. Anything that is still not a UUID is skipped and counted, never guessed at.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::IndexerError;

/// The hook-table kind these rows are stored as (0100's interest_signal_weights).
pub const KIND: &str = "not_interested";

/// geo-chat's rows, joined to the personal space. Read-only; the role needs SELECT on
/// `claim_not_interested (user_id, claim_entity_id, created_at)` and `users (id, profile_space_id)`.
pub const GEO_CHAT_QUERY: &str = "SELECT u.profile_space_id, n.claim_entity_id, n.created_at \
     FROM claim_not_interested n JOIN users u ON u.id = n.user_id";

/// The app role's statement_timeout is 30s; the recompute after a large first snapshot can exceed
/// it. `SET LOCAL`, inside the transaction, in case the connection goes through a pooler.
const APPLY_TIMEOUT: &str = "SET LOCAL statement_timeout = '5min'";

/// One row as geo-chat holds it.
#[derive(Debug, Clone, PartialEq, Deserialize, sqlx::FromRow)]
pub struct SourceRow {
    pub profile_space_id: String,
    pub claim_entity_id: String,
    pub created_at: DateTime<Utc>,
}

/// One row as gaia stores it.
#[derive(Debug, Clone, PartialEq)]
pub struct Signal {
    pub user_id: Uuid,
    pub object_id: Uuid,
    pub occurred_at: DateTime<Utc>,
}

/// Parses a Geo id in any of the spellings geo-chat accepts. `None` if it is not one.
pub fn parse_geo_id(value: &str) -> Option<Uuid> {
    let lower = value.trim().to_ascii_lowercase();
    let hex = lower.strip_prefix("0x").unwrap_or(&lower);
    let hex = if hex.len() == 64
        && hex.chars().all(|c| c.is_ascii_hexdigit())
        && hex[32..].chars().all(|c| c == '0')
    {
        &hex[..32]
    } else {
        hex
    };
    Uuid::parse_str(hex).ok()
}

/// The rows that parse, and how many did not.
pub fn to_signals(rows: &[SourceRow]) -> (Vec<Signal>, usize) {
    let mut signals = Vec::with_capacity(rows.len());
    let mut unparseable = 0;
    for row in rows {
        match (
            parse_geo_id(&row.profile_space_id),
            parse_geo_id(&row.claim_entity_id),
        ) {
            (Some(user_id), Some(object_id)) => signals.push(Signal {
                user_id,
                object_id,
                occurred_at: row.created_at,
            }),
            _ => unparseable += 1,
        }
    }
    (signals, unparseable)
}

/// Parses `--input` rows: JSON Lines with the geo-chat query's columns. A malformed line is an
/// error, never a silently shorter snapshot (which would read as undoing those marks).
pub fn parse_jsonl(body: &str) -> Result<Vec<SourceRow>, IndexerError> {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_str::<SourceRow>(line).map_err(|e| {
                IndexerError::Config(format!("unreadable Not interested row {line:?}: {e}"))
            })
        })
        .collect()
}

/// Reads the current set from geo-chat, in a read-only transaction.
pub async fn read_geo_chat(pool: &PgPool) -> Result<Vec<SourceRow>, IndexerError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION READ ONLY")
        .execute(&mut *tx)
        .await?;
    let rows = sqlx::query_as::<_, SourceRow>(GEO_CHAT_QUERY)
        .fetch_all(&mut *tx)
        .await?;
    tx.rollback().await?;
    Ok(rows)
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ApplyReport {
    pub rows_in: i32,
    /// Rows whose user is a personal space gaia knows.
    pub placed: i32,
    pub inserted: i32,
    pub updated: i32,
    pub removed: i32,
    pub users_recomputed: i32,
    pub signal_rows: i32,
}

/// Replaces gaia's `not_interested` rows with `signals` and recomputes the users that changed, in
/// one transaction. `dry_run` does all of it and rolls back, so the report is exactly what a real
/// run would do. `as_of` is for tests; the CronJob passes `None` (the database's `now()`).
pub async fn apply(
    pool: &PgPool,
    signals: &[Signal],
    allow_unplaced: bool,
    dry_run: bool,
    as_of: Option<DateTime<Utc>>,
) -> Result<ApplyReport, IndexerError> {
    let rows = serde_json::Value::Array(
        signals
            .iter()
            .map(|s| {
                serde_json::json!({
                    "user_id": s.user_id.to_string(),
                    "object_id": s.object_id.to_string(),
                    "occurred_at": s.occurred_at.to_rfc3339(),
                })
            })
            .collect(),
    );
    let mut tx = pool.begin().await?;
    sqlx::query(APPLY_TIMEOUT).execute(&mut *tx).await?;
    let report = sqlx::query_as::<_, ApplyReport>(
        "SELECT rows_in, placed, inserted, updated, removed, users_recomputed, signal_rows \
         FROM personalization.replace_external_interest_signals($1, $2::jsonb, $3, coalesce($4, now()))",
    )
    .bind(KIND)
    .bind(rows)
    .bind(allow_unplaced)
    .bind(as_of)
    .fetch_one(&mut *tx)
    .await?;
    if dry_run {
        tx.rollback().await?;
    } else {
        tx.commit().await?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0b9b1a35-2068-4431-8f7d-2350f958a728";

    #[test]
    fn parses_every_spelling_geo_chat_accepts() {
        let want = Uuid::parse_str(ID).unwrap();
        for spelling in [
            ID.to_string(),
            ID.replace('-', ""),
            format!(" {} ", ID.to_uppercase()),
            format!("0x{}", ID.replace('-', "")),
            format!("0x{}{}", ID.replace('-', ""), "0".repeat(32)),
        ] {
            assert_eq!(parse_geo_id(&spelling), Some(want), "{spelling}");
        }
    }

    #[test]
    fn refuses_what_is_not_an_id() {
        for bad in [
            "",
            "not-an-id",
            "0x1234",
            &format!("{}{}", ID.replace('-', ""), "1".repeat(32)),
        ] {
            assert_eq!(parse_geo_id(bad), None, "{bad}");
        }
    }

    #[test]
    fn skips_and_counts_rows_that_do_not_parse() {
        let at = DateTime::parse_from_rfc3339("2026-10-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let rows = vec![
            SourceRow {
                profile_space_id: ID.replace('-', ""),
                claim_entity_id: ID.into(),
                created_at: at,
            },
            SourceRow {
                profile_space_id: "nope".into(),
                claim_entity_id: ID.into(),
                created_at: at,
            },
        ];
        let (signals, unparseable) = to_signals(&rows);
        assert_eq!(unparseable, 1);
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].occurred_at, at);
    }

    #[test]
    fn a_malformed_input_line_is_an_error_not_a_shorter_snapshot() {
        let good = format!(
            "{{\"profile_space_id\":\"{ID}\",\"claim_entity_id\":\"{ID}\",\"created_at\":\"2026-10-01T00:00:00Z\"}}"
        );
        assert_eq!(parse_jsonl(&format!("{good}\n\n")).unwrap().len(), 1);
        assert!(parse_jsonl(&format!("{good}\nnot json\n")).is_err());
    }
}
