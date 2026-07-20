//! Checks whether a recovered `ipfs_cache` edit is safe to replay with
//! `replay_edit` — i.e. whether any *later* real edit has already touched
//! the same (entity, property, space) or (relation, space) targets.
//!
//! `replay_edit`'s underlying write path (`Storage::insert_value_versions` /
//! `insert_relation_versions`) unconditionally closes whatever version is
//! *currently open* for a touched target, setting its `valid_to_key` to this
//! edit's own `version_key` — regardless of chronological order. If a newer
//! edit already opened a version for that same target, replaying an older
//! edit would set `valid_to_key` to a value *earlier* than that version's
//! own `valid_from_key`, corrupting the version range and effectively
//! back-dating a stomp over legitimately newer data.
//!
//! This tool decodes the edit, runs it through the exact same `handle_edit`
//! extraction `replay_edit` uses (so the target set matches precisely what
//! would actually be written), and batch-queries the currently open version
//! for every touched target in one round trip per table — essential for
//! edits with tens of thousands of ops, where a per-target query would be
//! far too slow.
//!
//! Read-only: makes no writes. Exit code is 0 if safe to replay, 1 if any
//! conflict was found (details printed either way).
//!
//! Usage:
//!   DATABASE_URL=... cargo run -p kg-indexer --bin check_replay_safety -- \
//!     --uri ipfs://Qm... \
//!     --space <uuid> \
//!     --block <n> \
//!     [--sequence 0]

use std::collections::HashSet;
use std::env;

use grc_20::decode_edit;
use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;
use hermes_schema::pb::knowledge::HermesEdit;
use kg_indexer::error::IndexerError;
use kg_indexer::handlers;
use kg_indexer::storage::Storage;
use uuid::Uuid;

struct Args {
    uri: String,
    space: Uuid,
    block: u64,
    sequence: u32,
}

fn parse_args() -> Args {
    let args: Vec<String> = env::args().collect();
    let get = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    let uri = get("--uri").expect("--uri is required");
    let space = get("--space")
        .expect("--space is required")
        .parse()
        .expect("--space must be a valid UUID");
    let block: u64 = get("--block")
        .expect("--block is required")
        .parse()
        .expect("--block must be a number");
    let sequence: u32 = get("--sequence")
        .map(|s| s.parse().expect("--sequence must be a number"))
        .unwrap_or(0);

    Args {
        uri,
        space,
        block,
        sequence,
    }
}

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = parse_args();
    let version_key = ((args.block as i64) << 32) | args.sequence as i64;

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let storage = Storage::new(&database_url).await?;

    let row: (Option<Vec<u8>>, bool) =
        sqlx::query_as("SELECT data, is_errored FROM ipfs_cache WHERE uri = $1")
            .bind(&args.uri)
            .fetch_one(&storage.pool)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => {
                    IndexerError::Config(format!("uri not found in ipfs_cache: {}", args.uri))
                }
                other => IndexerError::Database(other),
            })?;
    let (data, is_errored) = row;
    if is_errored {
        return Err(IndexerError::Config(
            "ipfs_cache row is still marked is_errored — fix that first".into(),
        ));
    }
    let payload = data.ok_or_else(|| IndexerError::Config("ipfs_cache row has no data".into()))?;

    let decoded = decode_edit(&payload)
        .map_err(|e| IndexerError::Config(format!("payload does not decode: {e}")))?;

    let edit = HermesEdit {
        id: decoded.id.to_vec(),
        name: decoded.name.to_string(),
        payload: payload.clone(),
        authors: decoded.authors.iter().map(|a| a.to_vec()).collect(),
        language: None,
        space_id: args.space.as_bytes().to_vec(),
        is_canonical: true,
        meta: Some(BlockchainMetadata {
            created_at: 0,
            created_by: vec![],
            block_number: args.block,
            cursor: String::new(),
            sequence: args.sequence,
            is_last: false,
        }),
    };

    let result = handlers::edits::handle_edit(&edit)?;

    println!(
        "Edit {:?}: {} ops touching {} value targets, {} relation targets. version_key={version_key}",
        decoded.name,
        decoded.ops.len(),
        result.values.len(),
        result.relations.len(),
    );

    // Dedup targets — the same (entity, property, space) can be touched by
    // multiple ops (e.g. set then later unset within the same edit). Kept as
    // a single Vec<tuple> (not three parallel Vecs each derived from a
    // separate HashSet::iter() pass) so the three per-column arrays bound
    // below are built from one aligned iteration — `UNNEST` zips its
    // argument arrays by index, so any independent derivation risks
    // misaligning them and silently checking the wrong tuples.
    let mut seen_values = HashSet::new();
    let value_targets: Vec<(Uuid, Uuid, Uuid)> = result
        .values
        .iter()
        .map(|v| (v.entity_id, v.property_id, v.space_id))
        .filter(|t| seen_values.insert(*t))
        .collect();

    let mut seen_relations = HashSet::new();
    let relation_targets: Vec<(Uuid, Uuid)> = result
        .relations
        .iter()
        .map(|r| (r.id(), r.space_id()))
        .filter(|t| seen_relations.insert(*t))
        .collect();

    let mut conflicts: Vec<String> = Vec::new();

    if !value_targets.is_empty() {
        let mut entity_ids = Vec::with_capacity(value_targets.len());
        let mut property_ids = Vec::with_capacity(value_targets.len());
        let mut space_ids = Vec::with_capacity(value_targets.len());
        for (entity_id, property_id, space_id) in &value_targets {
            entity_ids.push(*entity_id);
            property_ids.push(*property_id);
            space_ids.push(*space_id);
        }

        let open_versions: Vec<(Uuid, Uuid, Uuid, i64)> = sqlx::query_as(
            r#"
            SELECT entity_id, property_id, space_id, valid_from_key
            FROM value_versions
            WHERE valid_to_key IS NULL
              AND (entity_id, property_id, space_id) IN (
                SELECT entity_id, property_id, space_id
                FROM UNNEST($1::uuid[], $2::uuid[], $3::uuid[])
                AS t(entity_id, property_id, space_id)
              )
            "#,
        )
        .bind(&entity_ids)
        .bind(&property_ids)
        .bind(&space_ids)
        .fetch_all(&storage.pool)
        .await?;

        for (entity_id, property_id, space_id, open_from_key) in open_versions {
            if open_from_key > version_key {
                conflicts.push(format!(
                    "VALUE CONFLICT: (entity={entity_id}, property={property_id}, space={space_id}) \
                     already has an open version from key {open_from_key} > this edit's {version_key} \
                     — replaying would corrupt that row's valid_to_key"
                ));
            }
        }
    }

    if !relation_targets.is_empty() {
        let mut relation_ids = Vec::with_capacity(relation_targets.len());
        let mut space_ids = Vec::with_capacity(relation_targets.len());
        for (relation_id, space_id) in &relation_targets {
            relation_ids.push(*relation_id);
            space_ids.push(*space_id);
        }

        let open_versions: Vec<(Uuid, Uuid, i64)> = sqlx::query_as(
            r#"
            SELECT relation_id, space_id, valid_from_key
            FROM relation_versions
            WHERE valid_to_key IS NULL
              AND (relation_id, space_id) IN (
                SELECT relation_id, space_id
                FROM UNNEST($1::uuid[], $2::uuid[])
                AS t(relation_id, space_id)
              )
            "#,
        )
        .bind(&relation_ids)
        .bind(&space_ids)
        .fetch_all(&storage.pool)
        .await?;

        for (relation_id, space_id, open_from_key) in open_versions {
            if open_from_key > version_key {
                conflicts.push(format!(
                    "RELATION CONFLICT: (relation={relation_id}, space={space_id}) already has an \
                     open version from key {open_from_key} > this edit's {version_key} — replaying \
                     would corrupt that row's valid_to_key"
                ));
            }
        }
    }

    if conflicts.is_empty() {
        println!(
            "SAFE — no later edit has touched any of this edit's {} targets.",
            value_targets.len() + relation_targets.len()
        );
        Ok(())
    } else {
        println!("UNSAFE — {} conflict(s) found:", conflicts.len());
        for c in &conflicts {
            println!("  {c}");
        }
        std::process::exit(1);
    }
}
