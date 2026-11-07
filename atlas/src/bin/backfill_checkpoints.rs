//! Copies the `atlas_checkpoints` rows into the blob store, one object per indexer id
//! (plan 10.22 section 6).
//!
//! Read-only against Postgres; writes only `atlas_checkpoints/<escaped indexer id>` objects in
//! the bucket named by the `BLOB_*` environment (credentials come from the `geodb-blob-key`
//! Secret via `secretKeyRef`, never read by hand). Safe to re-run: an object that already reads
//! back equal is skipped. Run it BEFORE turning the atlas write-through flag on, so the live
//! writer only ever replaces an object this tool created. Prints counts only.
//!
//! Usage:
//!   ATLAS_CHECKPOINT_DATABASE_URL=... BLOB_BACKEND=s3 BLOB_S3_*=... backfill_checkpoints \
//!     [--dry-run] [--repair]
//!
//! Exit codes: 0 every checkpoint present and equal, 1 one could not be read or verified,
//! 2 objects exist that differ from Postgres and were left alone (use `--repair` to replace).

use std::env;

use atlas::persistence::{
    backfill_checkpoint, CheckpointBackfill, PostgresCheckpointStore, PostgresPoolConfig,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let repair = args.iter().any(|a| a == "--repair");

    let database_url = env::var("ATLAS_CHECKPOINT_DATABASE_URL")
        .expect("ATLAS_CHECKPOINT_DATABASE_URL must be set");
    let Some(store) = blob_store::store_from_env()? else {
        anyhow::bail!("BLOB_BACKEND is unset; refusing to run without a target store");
    };
    let mirror = blob_store::Mirror::new(
        "atlas_checkpoints",
        store,
        blob_store::MirrorFlags {
            write_through: true,
            shadow_read: true,
        },
    );
    let pg = PostgresCheckpointStore::new(
        &database_url,
        &PostgresPoolConfig {
            max_connections: 2,
            min_connections: 0,
            acquire_timeout_ms: 10_000,
            idle_timeout_ms: 30_000,
            max_lifetime_ms: 300_000,
            statement_timeout_ms: 120_000,
        },
    )?;

    let ids = pg.indexer_ids().await?;
    let (mut written, mut would, mut unchanged, mut repaired, mut differs, mut failed) =
        (0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
    for id in &ids {
        let Some(checkpoint) = pg.load(id).await? else {
            // Deleted between the list and the load; nothing to copy.
            continue;
        };
        match backfill_checkpoint(&mirror, &checkpoint, repair, dry_run).await {
            CheckpointBackfill::Written => written += 1,
            CheckpointBackfill::WouldWrite => would += 1,
            CheckpointBackfill::Unchanged => unchanged += 1,
            CheckpointBackfill::Repaired => repaired += 1,
            CheckpointBackfill::Differs => differs += 1,
            CheckpointBackfill::Failed => failed += 1,
        }
    }
    println!(
        "done checkpoints={} written={written} would_write={would} unchanged={unchanged} \
         repaired={repaired} differs={differs} failed={failed}{}",
        ids.len(),
        if dry_run {
            " (dry run, nothing written)"
        } else {
            ""
        }
    );
    std::process::exit(if failed > 0 {
        1
    } else if differs > 0 {
        2
    } else {
        0
    });
}
