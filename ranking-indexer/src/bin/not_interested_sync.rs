//! Syncs "Not interested" on claims from geo-chat into gaia's topic-interest model (GEO-3088,
//! migration 0105). See `ranking_indexer::not_interested` for the mapping.
//!
//! Every couple of minutes: read geo-chat's whole `claim_not_interested` set (read-only), replace
//! gaia's `personalization.external_interest_signals` rows of kind `not_interested` with it, and
//! recompute every user whose set changed, so a mark or an undo moves that user's weights within
//! one run. Its own CronJob: a geo-chat outage fails this Job and nothing else.
//!
//! Exits non-zero WITHOUT writing when:
//!   * geo-chat is unreachable or the read fails;
//!   * geo-chat returned rows but none of them is usable — no id parses, or (checked in SQL) no
//!     user is a gaia personal space — while gaia holds rows: the identity mapping is broken, and
//!     acting on it would erase everyone's Not interested. `--allow-unplaced` overrides.
//!     An EMPTY read is honoured: it means everyone cleared their marks;
//!   * an `--input` line is malformed.
//!
//! `--dry-run` (or NOT_INTERESTED_SYNC_DRY_RUN=1) runs the whole replacement and rolls it back, so
//! the counts it prints are exactly what a real run would do.
//!
//! Input: geo-chat's Postgres (GEO_CHAT_DATABASE_URL, a read-only role), or `--input <file>` with
//! rows as JSON Lines `{"profile_space_id", "claim_entity_id", "created_at"}` for an offline run.
//!
//! Usage:
//!   DATABASE_URL=... GEO_CHAT_DATABASE_URL=... not_interested_sync [--dry-run] [--allow-unplaced] [--input rows.jsonl]

use std::env;
use std::time::Instant;

use sqlx::postgres::PgPoolOptions;
use tracing::{info, warn};

use ranking_indexer::error::IndexerError;
use ranking_indexer::not_interested;

#[derive(Debug, Default, PartialEq)]
struct Options {
    dry_run: bool,
    allow_unplaced: bool,
    input: Option<String>,
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Options, IndexerError> {
    let mut options = Options {
        dry_run: matches!(
            env::var("NOT_INTERESTED_SYNC_DRY_RUN").as_deref(),
            Ok("1") | Ok("true") | Ok("yes")
        ),
        ..Options::default()
    };
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dry-run" => options.dry_run = true,
            "--allow-unplaced" => options.allow_unplaced = true,
            "--input" => {
                options.input = Some(args.next().ok_or_else(|| {
                    IndexerError::Config("--input needs a file path".into())
                })?)
            }
            other => {
                return Err(IndexerError::Config(format!(
                    "unknown argument {other}; usage: not_interested_sync [--dry-run] [--allow-unplaced] [--input rows.jsonl]"
                )))
            }
        }
    }
    Ok(options)
}

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let options = parse_args(env::args().skip(1))?;
    let started = Instant::now();

    let rows = match &options.input {
        Some(path) => not_interested::parse_jsonl(
            &std::fs::read_to_string(path)
                .map_err(|e| IndexerError::Config(format!("cannot read {path}: {e}")))?,
        )?,
        None => {
            let url = env::var("GEO_CHAT_DATABASE_URL").map_err(|_| {
                IndexerError::Config("GEO_CHAT_DATABASE_URL not set (or pass --input)".into())
            })?;
            let geo_chat = PgPoolOptions::new()
                .max_connections(1)
                .connect(&url)
                .await?;
            let rows = not_interested::read_geo_chat(&geo_chat).await?;
            geo_chat.close().await;
            rows
        }
    };
    let (signals, unparseable) = not_interested::to_signals(&rows);
    if !rows.is_empty() && signals.is_empty() && !options.allow_unplaced {
        return Err(IndexerError::Config(format!(
            "none of geo-chat's {} Not interested rows has a Geo id; refusing to act on it (--allow-unplaced overrides)",
            rows.len()
        )));
    }
    if unparseable > 0 {
        warn!(
            unparseable,
            "Not interested rows with an id that is not a Geo id were skipped"
        );
    }

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;
    // Any error (including the SQL's refusal of a wholly unplaced snapshot) returns Err: the Job fails, and
    // nothing was written.
    let r = not_interested::apply(
        &pool,
        &signals,
        options.allow_unplaced,
        options.dry_run,
        None,
    )
    .await?;
    info!(
        source_rows = rows.len(),
        unparseable,
        rows_in = r.rows_in,
        placed = r.placed,
        unplaced = r.rows_in - r.placed,
        inserted = r.inserted,
        updated = r.updated,
        removed = r.removed,
        users_recomputed = r.users_recomputed,
        signal_rows = r.signal_rows,
        dry_run = options.dry_run,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "Not interested synced"
    );
    if options.dry_run {
        println!(
            "Dry run, rolled back: {} rows from geo-chat ({} unparseable), {} on a personal space; would insert {}, update {}, remove {}, and recompute {} users",
            rows.len(),
            unparseable,
            r.placed,
            r.inserted,
            r.updated,
            r.removed,
            r.users_recomputed
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments() {
        let o = parse_args(
            ["--dry-run", "--allow-unplaced", "--input", "f.jsonl"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        assert!(o.dry_run && o.allow_unplaced);
        assert_eq!(o.input.as_deref(), Some("f.jsonl"));
        assert!(parse_args(["--bogus".to_string()].into_iter()).is_err());
        assert!(parse_args(["--input".to_string()].into_iter()).is_err());
    }
}
