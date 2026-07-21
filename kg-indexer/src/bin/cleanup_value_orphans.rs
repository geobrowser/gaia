//! One-off retroactive cleanup for the 2026-07-21 space-misattribution
//! incident: `fix_misattribution`'s original live-sync logic decided which
//! row to sync a live `values` row to using a snapshot of "now open" state
//! taken *before* the version-table deletes ran. When a target's only
//! wrong-space history was the row being deleted, that snapshot pointed at
//! exactly the row about to be deleted — so the sync silently affected zero
//! rows (the JOIN found nothing), leaving a stale orphan live `values` row
//! behind with no backing `value_versions` history at all. Confirmed via a
//! Copilot review finding + direct DB check. Fixed properly in
//! `fix_misattribution` (now re-derives "now open" after the mutations run,
//! inside a transaction), but that fix doesn't retroactively clean up
//! orphans left behind by the buggy first pass — hence this tool.
//!
//! Scoped to `values` only (not `relations`): a value's live id is derived
//! from (entity_id, property_id, space_id), so an orphan check is entirely
//! self-contained per exact triple — no risk of only checking one space's
//! data and wrongly concluding a relation with legitimate history in a
//! *different* space is an orphan (which is exactly why the general-purpose
//! `repair_version_chains`, when pointed at only the wrong space, is NOT
//! safe to use for this — its relation-orphan logic aggregates by relation_id
//! across every space it was asked to check, and wrongly flags relations
//! whose live row correctly lives in a space this batch never queries).
//! Also does NOT touch `edit_versions` — these edits have already been
//! successfully replayed into their correct space, and re-deriving/deleting
//! their edit_versions rows here would undo that.
//!
//! For every (entity_id, property_id, wrong_space) target derived from the
//! given batch: if a live `values` row exists with the corresponding
//! deterministic id, and there is NO `value_versions` row at all for that
//! exact (entity_id, property_id, space_id) triple, delete the live row.
//!
//! Defaults to `--dry-run`. Pass `--execute` to actually apply.
//!
//! Usage:
//!   DATABASE_URL=... cargo run -p kg-indexer --bin cleanup_value_orphans -- \
//!     --batch-file /path/to/uri_block_wrongspace_lines.txt \
//!     [--execute]

use std::collections::HashSet;
use std::env;
use std::fs;

use grc_20::decode_edit;
use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;
use hermes_schema::pb::knowledge::HermesEdit;
use kg_indexer::error::IndexerError;
use kg_indexer::handlers;
use kg_indexer::storage::Storage;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args: Vec<String> = env::args().collect();
    let batch_file = args
        .iter()
        .position(|a| a == "--batch-file")
        .and_then(|i| args.get(i + 1))
        .expect("--batch-file is required");
    let execute = args.iter().any(|a| a == "--execute");

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let storage = Storage::new(&database_url).await?;

    let content = fs::read_to_string(batch_file)
        .map_err(|e| IndexerError::Config(format!("could not read batch file: {e}")))?;

    let mut value_targets: HashSet<(Uuid, Uuid, Uuid)> = HashSet::new();
    let mut decoded = 0usize;
    let mut skipped = 0usize;

    for line in content.lines().filter(|l| !l.trim().is_empty()) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            return Err(IndexerError::Config(format!(
                "malformed batch-file line (expected \"<uri> <block> <space> ...\"): {line:?}"
            )));
        }
        let (uri, block_str, space_str) = (parts[0], parts[1], parts[2]);
        let block: u64 = block_str
            .parse()
            .map_err(|e| IndexerError::Config(format!("bad block in line {line:?}: {e}")))?;
        let space: Uuid = space_str
            .parse()
            .map_err(|e| IndexerError::Config(format!("bad space in line {line:?}: {e}")))?;

        let row: (Option<Vec<u8>>, bool) =
            sqlx::query_as("SELECT data, is_errored FROM ipfs_cache WHERE uri = $1")
                .bind(uri)
                .fetch_one(&storage.pool)
                .await
                .map_err(|e| match e {
                    sqlx::Error::RowNotFound => {
                        IndexerError::Config(format!("uri not found in ipfs_cache: {uri}"))
                    }
                    other => IndexerError::Database(other),
                })?;
        let (data, is_errored) = row;
        if is_errored {
            println!("SKIP {uri}: ipfs_cache row still marked is_errored");
            skipped += 1;
            continue;
        }
        let payload = match data {
            Some(p) => p,
            None => {
                println!("SKIP {uri}: no data");
                skipped += 1;
                continue;
            }
        };
        let decoded_edit = match decode_edit(&payload) {
            Ok(d) => d,
            Err(e) => {
                println!("SKIP {uri}: decode failed: {e}");
                skipped += 1;
                continue;
            }
        };

        let edit = HermesEdit {
            id: decoded_edit.id.to_vec(),
            name: decoded_edit.name.to_string(),
            payload: payload.clone(),
            authors: decoded_edit.authors.iter().map(|a| a.to_vec()).collect(),
            language: None,
            space_id: space.as_bytes().to_vec(),
            is_canonical: true,
            meta: Some(BlockchainMetadata {
                created_at: 0,
                created_by: vec![],
                block_number: block,
                cursor: String::new(),
                sequence: 0,
                is_last: false,
            }),
        };

        let result = handlers::edits::handle_edit(&edit)?;
        decoded += 1;
        for v in &result.values {
            value_targets.insert((v.entity_id, v.property_id, v.space_id));
        }
    }

    println!(
        "Decoded {decoded} edit(s), {skipped} skipped. {} distinct value target(s) to check.",
        value_targets.len()
    );

    let targets: Vec<(Uuid, Uuid, Uuid)> = value_targets.into_iter().collect();
    let (e, p, s): (Vec<Uuid>, Vec<Uuid>, Vec<Uuid>) =
        targets
            .iter()
            .fold((vec![], vec![], vec![]), |mut acc, &(e, p, s)| {
                acc.0.push(e);
                acc.1.push(p);
                acc.2.push(s);
                acc
            });

    // Which of these targets still have ANY value_versions row at all
    // (open or closed) — those are NOT orphans, leave them alone.
    let backed: Vec<(Uuid, Uuid, Uuid)> = if targets.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as(
            "SELECT DISTINCT entity_id, property_id, space_id FROM value_versions \
             WHERE (entity_id, property_id, space_id) IN ( \
               SELECT * FROM UNNEST($1::uuid[], $2::uuid[], $3::uuid[]) \
             )",
        )
        .bind(&e)
        .bind(&p)
        .bind(&s)
        .fetch_all(&storage.pool)
        .await?
    };
    let backed_set: HashSet<(Uuid, Uuid, Uuid)> = backed.into_iter().collect();

    let live_ids: Vec<String> = targets
        .iter()
        .map(|&(e, p, s)| handlers::edits::derive_value_id(&e, &p, &s).to_string())
        .collect();
    let existing_live: Vec<String> =
        sqlx::query_scalar("SELECT id FROM values WHERE id = ANY($1::text[])")
            .bind(&live_ids)
            .fetch_all(&storage.pool)
            .await?;
    let existing_live_set: HashSet<String> = existing_live.into_iter().collect();

    let mut delete_live_ids: Vec<String> = Vec::new();
    for &target @ (e, p, s) in &targets {
        if backed_set.contains(&target) {
            continue; // has real history, not an orphan
        }
        let live_id = handlers::edits::derive_value_id(&e, &p, &s).to_string();
        if existing_live_set.contains(&live_id) {
            delete_live_ids.push(live_id);
        }
    }

    println!(
        "\n{} orphaned live value row(s) found (zero backing value_versions history).",
        delete_live_ids.len()
    );

    if !execute {
        println!("\nDry run — no changes made. Pass --execute to apply.");
        return Ok(());
    }

    if !delete_live_ids.is_empty() {
        sqlx::query("DELETE FROM values WHERE id = ANY($1::text[])")
            .bind(&delete_live_ids)
            .execute(&storage.pool)
            .await?;
    }
    println!("\nExecuted successfully.");

    Ok(())
}
