//! Re-attempts every `ipfs_cache` row marked `is_errored = true` using the
//! now-fixed `IpfsClient` (retry-with-backoff + CID verification — see
//! `ipfs/src/verify.rs` and `ipfs/src/lib.rs::IpfsClient::get_bytes`).
//!
//! Before that fix, a single transient fetch failure (a truncated read, a
//! flaky gateway response) was cached as permanently errored with no retry
//! anywhere upstream — `hermes-pipeline` then silently and permanently
//! dropped the edit. A one-time system-wide audit (2026-07-17) found 211 of
//! 659 errored rows were exactly this: recoverable false positives, not
//! genuinely invalid content.
//!
//! This tool repairs the *cache* row only (`data` + `is_errored = false`).
//! It does NOT replay the edit into the live knowledge graph — a cache row
//! being wrong doesn't retroactively make `hermes-pipeline` reprocess an
//! edit it already dropped. For that, check the target (entity, property,
//! space) tuples for conflicting later edits by hand, then use
//! `kg-indexer/src/bin/replay_edit.rs` (mirrors this tool's caution).
//!
//! Safe to run repeatedly: only touches rows still marked `is_errored`, and
//! genuinely invalid content (fails to decode even once correctly fetched
//! and, where the CID shape is one the `ipfs` crate's internal verification
//! understands, hash-verified)
//! is left alone rather than looped on forever.
//!
//! Usage:
//!   DATABASE_URL=... IPFS_GATEWAY_URL=... cargo run -p hermes-ipfs-cache --bin reconcile_errored \
//!     -- [--dry-run] [--limit N]

use std::env;
use std::time::Duration;

use grc_20::decode_edit;
use ipfs::{IpfsClient, IpfsFetcher};
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args: Vec<String> = env::args().collect();
    let dry_run = args.iter().any(|a| a == "--dry-run");
    // Distinguish "--limit absent" (limit = None, sweep everything) from
    // "--limit present but missing/invalid value" (must error, not silently
    // fall back to sweeping everything).
    let limit: Option<i64> = args.iter().position(|a| a == "--limit").map(|i| {
        args.get(i + 1)
            .unwrap_or_else(|| panic!("--limit requires a value"))
            .parse()
            .expect("--limit must be a number")
    });

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let ipfs_gateway = env::var("IPFS_GATEWAY_URL").expect("IPFS_GATEWAY_URL must be set");

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;
    let ipfs = IpfsClient::new(&ipfs_gateway);

    let errored: Vec<(String,)> =
        sqlx::query_as("SELECT uri FROM ipfs_cache WHERE is_errored = true ORDER BY uri LIMIT $1")
            .bind(limit.unwrap_or(i64::MAX))
            .fetch_all(&pool)
            .await?;

    println!(
        "Found {} errored ipfs_cache row(s){}{}",
        errored.len(),
        if dry_run {
            " (dry run — no writes)"
        } else {
            ""
        },
        limit
            .map(|l| format!(" (limited to {l})"))
            .unwrap_or_default()
    );

    let mut recovered = 0u32;
    let mut still_failing = 0u32;

    for (uri,) in errored {
        match ipfs.get_bytes(&uri).await {
            Ok(bytes) => match decode_edit(&bytes) {
                Ok(edit) => {
                    let name = if edit.name.is_empty() {
                        None
                    } else {
                        Some(edit.name.into_owned())
                    };
                    println!(
                        "RECOVERABLE  {uri}  ({} ops, name={name:?})",
                        edit.ops.len()
                    );
                    recovered += 1;
                    if !dry_run {
                        sqlx::query(
                            "UPDATE ipfs_cache SET data = $1, is_errored = false, name = $2 \
                             WHERE uri = $3 AND is_errored = true",
                        )
                        .bind(&bytes)
                        .bind(&name)
                        .bind(&uri)
                        .execute(&pool)
                        .await?;
                    }
                }
                Err(decode_error) => {
                    println!("STILL INVALID  {uri}  (fetched successfully, but: {decode_error})");
                    still_failing += 1;
                }
            },
            Err(fetch_error) => {
                println!("STILL UNFETCHABLE  {uri}  ({fetch_error})");
                still_failing += 1;
            }
        }
        // Be polite to the gateway — this is a batch sweep, not latency-sensitive.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    println!(
        "\nDone. {recovered} recoverable{}, {still_failing} still failing.",
        if dry_run {
            " (not written — dry run)"
        } else {
            " (cache row fixed)"
        }
    );
    if recovered > 0 && !dry_run {
        println!(
            "Note: cache rows are fixed, but recovered edits are NOT yet in the live KG — \
             check version history for conflicts, then use kg-indexer's replay_edit tool per edit."
        );
    }

    Ok(())
}
