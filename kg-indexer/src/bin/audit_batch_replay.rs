//! One-off audit for the 2026-07-20 batch-replay-ordering incident: a batch
//! of recovered edits was replayed via `replay_edit` in alphabetical-URI
//! order rather than chronological block order. `replay_edit`'s write path
//! (mirroring the real Kafka consumer) always closes whatever version is
//! *currently open* for a touched target and treats its own edit as the new
//! latest — it has no awareness of other edits in the same batch. Any
//! target touched by more than one edit in a batch that wasn't replayed in
//! block order ends up with a corrupted `valid_to_key` chain.
//!
//! Given a list of (uri, block, space) triples (one per line, space-
//! separated, same format as the batch that was replayed), this:
//! 1. Decodes each edit and collects every (entity, property, space) and
//!    (relation, space) target it touches, same extraction as
//!    `check_replay_safety`.
//! 2. Groups by target. Every target the batch touches is checked — not
//!    just ones shared between 2+ batch edits, since a single batch edit
//!    can also corrupt or resurrect data over an *independent*, non-batch
//!    edit that touched the same target (see `check_replay_safety`'s
//!    Unset/Delete handling for why "no open row" doesn't mean "nothing
//!    else touched this").
//! 3. For each target, fetches the *entire* current version chain (not
//!    just the open row) and checks whether `valid_to_key` on each row
//!    equals the `valid_from_key` of the next row in ascending order (and
//!    the last row is open, i.e. `valid_to_key IS NULL`).
//! 4. Reports any chain that's out of order, with the exact corrected
//!    `valid_to_key` for each affected row. Read-only — prints proposed
//!    fixes, does not apply them.
//!
//! Usage:
//!   DATABASE_URL=... cargo run -p kg-indexer --bin audit_batch_replay -- \
//!     --batch-file /path/to/uri_block_space_lines.txt

use std::collections::HashMap;
use std::env;
use std::fs;

use grc_20::decode_edit;
use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;
use hermes_schema::pb::knowledge::HermesEdit;
use kg_indexer::error::IndexerError;
use kg_indexer::handlers;
use kg_indexer::storage::Storage;
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Target {
    Value(Uuid, Uuid, Uuid),
    Relation(Uuid, Uuid),
}

struct BatchEdit {
    uri: String,
    block: u64,
}

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

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let storage = Storage::new(&database_url).await?;

    let lines = fs::read_to_string(batch_file)
        .map_err(|e| IndexerError::Config(format!("could not read batch file: {e}")))?;

    // target -> list of (uri, block, version_key) that touch it
    let mut target_edits: HashMap<Target, Vec<BatchEdit>> = HashMap::new();

    for line in lines.lines().filter(|l| !l.trim().is_empty()) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() != 3 {
            return Err(IndexerError::Config(format!(
                "malformed batch-file line (expected \"<uri> <block> <space>\"): {line:?}"
            )));
        }
        let (uri, block_str, space_str) = (parts[0], parts[1], parts[2]);
        let block: u64 = block_str
            .parse()
            .map_err(|e| IndexerError::Config(format!("bad block number in line {line:?}: {e}")))?;
        let space: Uuid = space_str
            .parse()
            .map_err(|e| IndexerError::Config(format!("bad space UUID in line {line:?}: {e}")))?;

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
            return Err(IndexerError::Config(format!(
                "{uri}: ipfs_cache row is still marked is_errored — fix that first"
            )));
        }
        let payload = data.ok_or_else(|| IndexerError::Config("no data".into()))?;
        let decoded = decode_edit(&payload)
            .map_err(|e| IndexerError::Config(format!("decode failed: {e}")))?;

        let edit = HermesEdit {
            id: decoded.id.to_vec(),
            name: decoded.name.to_string(),
            payload: payload.clone(),
            authors: decoded.authors.iter().map(|a| a.to_vec()).collect(),
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

        for v in &result.values {
            target_edits
                .entry(Target::Value(v.entity_id, v.property_id, v.space_id))
                .or_default()
                .push(BatchEdit {
                    uri: uri.to_string(),
                    block,
                });
        }
        for r in &result.relations {
            target_edits
                .entry(Target::Relation(r.id(), r.space_id()))
                .or_default()
                .push(BatchEdit {
                    uri: uri.to_string(),
                    block,
                });
        }
    }

    // Check the full chain for every target the batch touched, not just
    // ones shared between 2+ batch edits — a single batch edit can also
    // silently resurrect data over an independent, non-batch edit that
    // later unset/deleted it (that edit closes a version without opening a
    // new one, so it wouldn't otherwise show up as a "shared target").
    println!(
        "Checking full version chain for {} target(s) touched by this batch.",
        target_edits.len()
    );

    let mut any_corrupt = false;

    for (target, edits) in target_edits.iter() {
        match target {
            Target::Value(entity_id, property_id, space_id) => {
                let rows: Vec<(Uuid, i64, Option<i64>)> = sqlx::query_as(
                    "SELECT id, valid_from_key, valid_to_key FROM value_versions \
                     WHERE entity_id = $1 AND property_id = $2 AND space_id = $3 \
                     ORDER BY valid_from_key",
                )
                .bind(entity_id)
                .bind(property_id)
                .bind(space_id)
                .fetch_all(&storage.pool)
                .await?;

                report_chain(
                    &format!(
                        "VALUE (entity={entity_id}, property={property_id}, space={space_id})"
                    ),
                    &rows,
                    edits,
                    &mut any_corrupt,
                );
                continue;
            }
            Target::Relation(relation_id, space_id) => {
                let rows: Vec<(Uuid, i64, Option<i64>)> = sqlx::query_as(
                    "SELECT id, valid_from_key, valid_to_key FROM relation_versions \
                     WHERE relation_id = $1 AND space_id = $2 ORDER BY valid_from_key",
                )
                .bind(relation_id)
                .bind(space_id)
                .fetch_all(&storage.pool)
                .await?;

                report_chain(
                    &format!("RELATION (relation={relation_id}, space={space_id})"),
                    &rows,
                    edits,
                    &mut any_corrupt,
                );
            }
        }
    }

    if !any_corrupt {
        println!("No out-of-order chains found among any of the batch's targets.");
    }

    Ok(())
}

fn report_chain(
    label: &str,
    rows: &[(Uuid, i64, Option<i64>)],
    batch_edits: &[BatchEdit],
    any_corrupt: &mut bool,
) {
    // A target's *last* row legitimately has no successor whenever the
    // final touch was an Unset/Delete — those close a version without
    // opening a new one (see Storage::insert_relation_versions/
    // insert_value_versions, which only insert new rows for Create/Set).
    // So the only real invariants are: (1) no row's valid_to is earlier
    // than its own valid_from (closed before it opened), and (2) every row
    // that *does* have a successor must hand off to it exactly (this row's
    // valid_to == the next row's valid_from). Whether the last row is open
    // or closed is not itself evidence of anything.
    let mut corrupt = false;
    let mut lines = Vec::new();
    for i in 0..rows.len() {
        let (id, valid_from, valid_to) = &rows[i];
        if let Some(vt) = valid_to {
            if vt < valid_from {
                corrupt = true;
            }
        }
        let expected_valid_to = rows.get(i + 1).map(|(_, next_from, _)| *next_from);
        if expected_valid_to.is_some() && *valid_to != expected_valid_to {
            corrupt = true;
        }
        lines.push(format!(
            "  row {id}: valid_from={valid_from} valid_to={valid_to:?} (expected {expected_valid_to:?})"
        ));
    }
    if corrupt {
        *any_corrupt = true;
        for line in &lines {
            println!("{line}");
        }
        println!(
            "CORRUPT CHAIN: {label} — touched by batch edits: {}",
            batch_edits
                .iter()
                .map(|e| format!("{}(block {})", e.uri, e.block))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}
