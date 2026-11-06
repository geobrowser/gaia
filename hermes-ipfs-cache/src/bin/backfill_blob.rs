//! Copies existing `ipfs_cache` rows into the blob store (plan 10.22 section 6).
//!
//! Read-only against Postgres (keyset scan by uri); writes only to the bucket named by the
//! `BLOB_*` environment (credentials come from the `geodb-blob-key` Secret via `secretKeyRef`,
//! never read by hand). Safe to re-run: an object that already reads back equal is skipped.
//! Prints counts and the resume position, never row contents.
//!
//! Usage:
//!   DATABASE_URL=... BLOB_BACKEND=s3 BLOB_S3_*=... backfill_blob \
//!     [--dry-run] [--repair] [--limit N] [--batch N] [--concurrency N] [--after URI]
//!
//! Exit codes: 0 every row present and equal, 1 a row could not be read or verified,
//! 2 objects exist that differ from Postgres and were left alone (use `--repair` to replace).

use std::env;
use std::sync::Arc;

use hermes_ipfs_cache::backfill::{Counts, backfill_mirror, sync_row};
use sqlx::postgres::PgPoolOptions;

/// `(uri, block, space, is_errored, name, data)` of one `ipfs_cache` row.
type CacheRow = (
    String,
    String,
    String,
    bool,
    Option<String>,
    Option<Vec<u8>>,
);

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).map(|i| {
        args.get(i + 1)
            .unwrap_or_else(|| panic!("{flag} requires a value"))
            .clone()
    })
}

fn arg_num(args: &[String], flag: &str) -> Option<i64> {
    arg_value(args, flag).map(|v| {
        v.parse()
            .unwrap_or_else(|_| panic!("{flag} must be a number"))
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args: Vec<String> = env::args().collect();
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let repair = args.iter().any(|a| a == "--repair");
    let limit = arg_num(&args, "--limit");
    let batch = arg_num(&args, "--batch").unwrap_or(50).max(1);
    let concurrency = arg_num(&args, "--concurrency").unwrap_or(8).max(1) as usize;
    let mut after = arg_value(&args, "--after").unwrap_or_default();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let mirror = match backfill_mirror()? {
        Some(m) => Arc::new(m),
        None => anyhow::bail!("BLOB_BACKEND is unset; refusing to run without a target store"),
    };
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await?;

    let mut counts = Counts::default();
    let mut seen: i64 = 0;
    let started = std::time::Instant::now();
    loop {
        let take = limit.map_or(batch, |l| batch.min(l - seen));
        if take <= 0 {
            break;
        }
        let rows: Vec<CacheRow> = sqlx::query_as(
            "SELECT uri, block, space::text, is_errored, name, data \
             FROM ipfs_cache WHERE uri > $1 ORDER BY uri LIMIT $2",
        )
        .bind(&after)
        .bind(take)
        .fetch_all(&pool)
        .await?;
        if rows.is_empty() {
            break;
        }
        seen += rows.len() as i64;
        if let Some(last) = rows.last() {
            after = last.0.clone();
        }

        let mut set = tokio::task::JoinSet::new();
        let mut rows = rows.into_iter();
        let mut running = 0usize;
        loop {
            while running < concurrency {
                let Some((uri, block, space, is_errored, name, data)) = rows.next() else {
                    break;
                };
                let mirror = mirror.clone();
                set.spawn(async move {
                    let row = blob_store::ipfs_object::Row {
                        block,
                        space,
                        is_errored,
                        name,
                        data,
                    };
                    sync_row(&mirror, &uri, &row, repair, dry_run).await
                });
                running += 1;
            }
            match set.join_next().await {
                Some(res) => {
                    running -= 1;
                    counts.add(res.map_err(|e| anyhow::anyhow!("worker panicked: {e}"))?);
                }
                None => break,
            }
        }

        if seen % (batch * 20) < batch {
            println!(
                "progress rows={seen} elapsed_s={} resume_after={after}",
                started.elapsed().as_secs()
            );
        }
    }

    println!(
        "done rows={} written={} would_write={} unchanged={} repaired={} differs={} failed={}{}",
        counts.total(),
        counts.written,
        counts.would_write,
        counts.unchanged,
        counts.repaired,
        counts.differs,
        counts.failed,
        if dry_run {
            " (dry run, nothing written)"
        } else {
            ""
        }
    );
    if counts.failed > 0 {
        println!("resume_after={after}");
    }
    std::process::exit(counts.exit_code());
}
