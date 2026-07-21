//! Generic version-chain repair for the 2026-07-21 backfill-corruption
//! incident: `replay_edit` (and any backfill tool built on
//! `insert_value_versions`/`insert_relation_versions`) always assumes the
//! version it's inserting is the newest for its target, and unconditionally
//! closes whatever's currently open. If a target already has a version with
//! a *larger* valid_from_key (e.g. backfilling an old block after real-time
//! indexing has already moved past it — see the shared "latest batch"
//! entities the news-worker keeps updating), this corrupts the linked list:
//! the wrong row gets closed, and the new backfilled row is left open even
//! though it's actually the *oldest*. Confirmed via `audit_batch_replay`.
//!
//! This is independent of how the corruption happened — the fix is a pure
//! invariant restoration: for each target, fetch its full chain, sort by
//! valid_from_key, and relink (each row's valid_to_key = next row's
//! valid_from_key; the last row is open). Then resync the live
//! `values`/`relations` row to whichever version is genuinely open
//! afterward, since `insert_values`/`insert_relations` unconditionally
//! overwrite live data with whatever was most recently *processed*
//! (regardless of temporal order), so live data may currently reflect a
//! stale backfilled row instead of newer legitimate content.
//!
//! Safe to run on an already-correct chain (no-op). Defaults to
//! `--dry-run`; pass `--execute` to apply.
//!
//! Usage:
//!   DATABASE_URL=... cargo run -p kg-indexer --bin repair_version_chains -- \
//!     --batch-file /path/to/uri_block_space_lines.txt \
//!     [--execute]

use std::collections::{HashMap, HashSet};
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

    let lines = fs::read_to_string(batch_file)
        .map_err(|e| IndexerError::Config(format!("could not read batch file: {e}")))?;

    // ---- Phase 1: decode every edit, collect its distinct targets ----
    let mut targets: HashSet<Target> = HashSet::new();
    let mut decoded_count = 0usize;
    let mut skipped = 0usize;

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
        let decoded = match decode_edit(&payload) {
            Ok(d) => d,
            Err(e) => {
                println!("SKIP {uri}: decode failed: {e}");
                skipped += 1;
                continue;
            }
        };

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
        decoded_count += 1;

        for v in &result.values {
            targets.insert(Target::Value(v.entity_id, v.property_id, v.space_id));
        }
        for r in &result.relations {
            targets.insert(Target::Relation(r.id(), r.space_id()));
        }
    }

    println!(
        "Decoded {decoded_count} edit(s), {skipped} skipped. {} distinct target(s) to check.",
        targets.len()
    );

    let value_targets: Vec<(Uuid, Uuid, Uuid)> = targets
        .iter()
        .filter_map(|t| match t {
            Target::Value(e, p, s) => Some((*e, *p, *s)),
            Target::Relation(..) => None,
        })
        .collect();
    let relation_targets: Vec<(Uuid, Uuid)> = targets
        .iter()
        .filter_map(|t| match t {
            Target::Relation(r, s) => Some((*r, *s)),
            Target::Value(..) => None,
        })
        .collect();

    // ---- Phase 2: batch-fetch the FULL chain for every target ----
    let (v_entity, v_property, v_space): (Vec<Uuid>, Vec<Uuid>, Vec<Uuid>) = value_targets
        .iter()
        .map(|(e, p, s)| (*e, *p, *s))
        .fold((vec![], vec![], vec![]), |mut acc, (e, p, s)| {
            acc.0.push(e);
            acc.1.push(p);
            acc.2.push(s);
            acc
        });
    let value_rows: Vec<(Uuid, Uuid, Uuid, Uuid, i64, Option<i64>)> = if value_targets.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as(
            "SELECT entity_id, property_id, space_id, id, valid_from_key, valid_to_key \
             FROM value_versions \
             WHERE (entity_id, property_id, space_id) IN ( \
               SELECT * FROM UNNEST($1::uuid[], $2::uuid[], $3::uuid[]) \
             )",
        )
        .bind(&v_entity)
        .bind(&v_property)
        .bind(&v_space)
        .fetch_all(&storage.pool)
        .await?
    };

    let (r_relation, r_space): (Vec<Uuid>, Vec<Uuid>) = relation_targets
        .iter()
        .map(|(r, s)| (*r, *s))
        .fold((vec![], vec![]), |mut acc, (r, s)| {
            acc.0.push(r);
            acc.1.push(s);
            acc
        });
    let relation_rows: Vec<(Uuid, Uuid, Uuid, i64, Option<i64>)> = if relation_targets.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as(
            "SELECT relation_id, space_id, id, valid_from_key, valid_to_key \
             FROM relation_versions \
             WHERE (relation_id, space_id) IN ( \
               SELECT * FROM UNNEST($1::uuid[], $2::uuid[]) \
             )",
        )
        .bind(&r_relation)
        .bind(&r_space)
        .fetch_all(&storage.pool)
        .await?
    };

    println!(
        "Fetched {} value_version row(s), {} relation_version row(s) across all targets.",
        value_rows.len(),
        relation_rows.len()
    );

    // ---- Phase 3: in-memory relink ----
    // Seeded from every decoded target (not just ones with query results) so
    // a target backed by zero version_versions rows still gets checked —
    // that's exactly the "live row is a pure orphan" case: no chain to
    // relink, but a stale live row may still need deleting.
    type ChainRow = (Uuid, i64, Option<i64>); // (id, valid_from_key, valid_to_key)
    let mut value_chains: HashMap<(Uuid, Uuid, Uuid), Vec<ChainRow>> =
        value_targets.iter().map(|&t| (t, Vec::new())).collect();
    for (e, p, s, id, vf, vt) in value_rows {
        value_chains
            .entry((e, p, s))
            .or_default()
            .push((id, vf, vt));
    }
    let mut relation_chains: HashMap<(Uuid, Uuid), Vec<ChainRow>> =
        relation_targets.iter().map(|&t| (t, Vec::new())).collect();
    for (r, s, id, vf, vt) in relation_rows {
        relation_chains
            .entry((r, s))
            .or_default()
            .push((id, vf, vt));
    }

    let mut update_value_valid_to: Vec<(Uuid, Option<i64>)> = Vec::new();
    let mut sync_live_value: Vec<(String, Uuid)> = Vec::new(); // (live_id, source_version_id)
    let mut delete_live_value_ids: Vec<String> = Vec::new();
    let mut corrupt_value_targets = 0usize;

    for ((entity_id, property_id, space_id), mut rows) in value_chains {
        rows.sort_by_key(|(_, vf, _)| *vf);
        let mut was_corrupt = false;
        for i in 0..rows.len() {
            let (id, _vf, vt) = rows[i];
            let correct_vt = rows.get(i + 1).map(|(_, next_vf, _)| *next_vf);
            if vt != correct_vt {
                was_corrupt = true;
                update_value_valid_to.push((id, correct_vt));
            }
        }
        if was_corrupt {
            corrupt_value_targets += 1;
        }
        // Always resync regardless of `was_corrupt`, not just when the
        // chain needed relinking — the live row can still be stale even
        // when its own version chain is already correctly ordered (e.g. it
        // was never resynced by whatever wrote the version rows).
        let live_id =
            handlers::edits::derive_value_id(&entity_id, &property_id, &space_id).to_string();
        match rows.last() {
            Some((open_id, _, _)) => {
                sync_live_value.push((live_id, *open_id));
            }
            None => {
                delete_live_value_ids.push(live_id);
            }
        }
    }

    let mut update_relation_valid_to: Vec<(Uuid, Option<i64>)> = Vec::new();
    let mut corrupt_relation_targets = 0usize;
    // relation_versions is partitioned by (relation_id, space_id), but the
    // live `relations` row is keyed globally by relation_id alone — and
    // `insert_relations`'s ON CONFLICT never updates `space_id` (by design,
    // to keep a relation's home space stable across ordinary updates). That
    // means once a relation's live row is created in the wrong space,
    // replaying it into the correct space's version chain does NOT fix the
    // live row on its own, even though that per-space chain isn't "corrupt"
    // in the linking sense. So live sync must be decided globally per
    // relation_id (which space currently has the open/latest row), not
    // gated on whether any single space's chain needed relinking.
    // Seeded from every distinct relation_id so one with zero backing rows
    // in any space still surfaces below (as an empty `opens` list) instead
    // of silently never appearing at all.
    let mut open_per_relation: HashMap<Uuid, Vec<(Uuid, Uuid)>> = relation_targets
        .iter()
        .map(|&(r, _)| (r, Vec::new()))
        .collect();

    for ((relation_id, space_id), mut rows) in relation_chains {
        rows.sort_by_key(|(_, vf, _)| *vf);
        let mut was_corrupt = false;
        for i in 0..rows.len() {
            let (id, _vf, vt) = rows[i];
            let correct_vt = rows.get(i + 1).map(|(_, next_vf, _)| *next_vf);
            if vt != correct_vt {
                was_corrupt = true;
                update_relation_valid_to.push((id, correct_vt));
            }
        }
        if was_corrupt {
            corrupt_relation_targets += 1;
        }
        if let Some((open_id, _, _)) = rows.last() {
            open_per_relation
                .entry(relation_id)
                .or_default()
                .push((space_id, *open_id));
        }
    }

    // Which spaces this run actually queried relation_versions for, per
    // relation_id — needed below to tell "confirmed no history anywhere" (0
    // opens found here would be conclusive if this were the only space)
    // apart from "we just didn't check the space this relation currently
    // lives in".
    let mut checked_spaces_by_relation: HashMap<Uuid, HashSet<Uuid>> = HashMap::new();
    for &(r, s) in &relation_targets {
        checked_spaces_by_relation.entry(r).or_default().insert(s);
    }

    let mut sync_live_relation: Vec<(Uuid, Uuid)> = Vec::new(); // (relation_id, source_version_id)
    let mut delete_live_relation_ids: Vec<Uuid> = Vec::new();
    let mut zero_open_relation_ids: Vec<Uuid> = Vec::new();

    for (relation_id, mut opens) in open_per_relation {
        match opens.len() {
            0 => zero_open_relation_ids.push(relation_id),
            1 => sync_live_relation.push((relation_id, opens.remove(0).1)),
            _ => {
                println!(
                    "  WARNING: relation {relation_id} has an open version in {} different spaces \
                     ({opens:?}) — ambiguous, leaving live row untouched.",
                    opens.len()
                );
            }
        }
    }

    // A relation with zero opens among the spaces this run checked is only
    // a confirmed orphan if its CURRENT live space_id is one of those we
    // actually queried. If it currently lives in some other space this run
    // never looked at, deleting it would drop valid history purely because
    // we didn't check — so leave it untouched instead of guessing.
    if !zero_open_relation_ids.is_empty() {
        let current_relation_spaces: Vec<(Uuid, Option<Uuid>)> =
            sqlx::query_as("SELECT id, space_id FROM relations WHERE id = ANY($1::uuid[])")
                .bind(&zero_open_relation_ids)
                .fetch_all(&storage.pool)
                .await?;
        let current_space_by_relation: HashMap<Uuid, Option<Uuid>> =
            current_relation_spaces.into_iter().collect();

        for relation_id in zero_open_relation_ids {
            let current_space = current_space_by_relation
                .get(&relation_id)
                .copied()
                .flatten();
            match current_space {
                None => {} // no live row exists at all — nothing to delete
                Some(space) => {
                    let checked = checked_spaces_by_relation
                        .get(&relation_id)
                        .is_some_and(|spaces| spaces.contains(&space));
                    if checked {
                        delete_live_relation_ids.push(relation_id);
                    } else {
                        println!(
                            "  WARNING: relation {relation_id} has no open version in the \
                             space(s) this run checked, but its live row currently sits in \
                             {space}, which wasn't checked — leaving it untouched rather than \
                             risk deleting valid history this run never queried."
                        );
                    }
                }
            }
        }
    }

    println!(
        "\n{corrupt_value_targets} value target(s) and {corrupt_relation_targets} relation target(s) had a corrupt chain.\n\
         Plan: update {} value_version row(s), resync {} live value row(s), delete {} live value row(s); \
         update {} relation_version row(s), resync {} live relation row(s), delete {} live relation row(s).",
        update_value_valid_to.len(),
        sync_live_value.len(),
        delete_live_value_ids.len(),
        update_relation_valid_to.len(),
        sync_live_relation.len(),
        delete_live_relation_ids.len(),
    );

    if !execute {
        println!("\nDry run — no changes made. Pass --execute to apply.");
        return Ok(());
    }

    let mut tx = storage.pool.begin().await?;

    if !update_value_valid_to.is_empty() {
        let (ids, valid_tos): (Vec<Uuid>, Vec<Option<i64>>) =
            update_value_valid_to.into_iter().unzip();
        sqlx::query(
            "UPDATE value_versions vv SET valid_to_key = u.valid_to_key \
             FROM UNNEST($1::uuid[], $2::bigint[]) AS u(id, valid_to_key) WHERE vv.id = u.id",
        )
        .bind(&ids)
        .bind(&valid_tos)
        .execute(&mut *tx)
        .await?;
    }
    if !sync_live_value.is_empty() {
        let (live_ids, source_ids): (Vec<String>, Vec<Uuid>) = sync_live_value.into_iter().unzip();
        sqlx::query(
            "INSERT INTO values (id, entity_id, property_id, space_id, text, language, unit, boolean, \
             decimal, point, time, integer, float, bytes, date, datetime, schedule, embedding, \
             time_utc, datetime_utc, rect) \
             SELECT u.live_id, vv.entity_id, vv.property_id, vv.space_id, vv.text, vv.language, vv.unit, \
             vv.boolean, vv.decimal, vv.point, vv.time, vv.integer, vv.float, vv.bytes, vv.date, \
             vv.datetime, vv.schedule, vv.embedding, vv.time_utc, vv.datetime_utc, vv.rect \
             FROM UNNEST($1::text[], $2::uuid[]) AS u(live_id, source_id) \
             JOIN value_versions vv ON vv.id = u.source_id \
             ON CONFLICT (id) DO UPDATE SET text = EXCLUDED.text, language = EXCLUDED.language, \
             unit = EXCLUDED.unit, boolean = EXCLUDED.boolean, decimal = EXCLUDED.decimal, \
             point = EXCLUDED.point, time = EXCLUDED.time, integer = EXCLUDED.integer, \
             float = EXCLUDED.float, bytes = EXCLUDED.bytes, date = EXCLUDED.date, \
             datetime = EXCLUDED.datetime, schedule = EXCLUDED.schedule, embedding = EXCLUDED.embedding, \
             time_utc = EXCLUDED.time_utc, datetime_utc = EXCLUDED.datetime_utc, rect = EXCLUDED.rect",
        )
        .bind(&live_ids)
        .bind(&source_ids)
        .execute(&mut *tx)
        .await?;
    }
    if !delete_live_value_ids.is_empty() {
        sqlx::query("DELETE FROM values WHERE id = ANY($1::text[])")
            .bind(&delete_live_value_ids)
            .execute(&mut *tx)
            .await?;
    }

    if !update_relation_valid_to.is_empty() {
        let (ids, valid_tos): (Vec<Uuid>, Vec<Option<i64>>) =
            update_relation_valid_to.into_iter().unzip();
        sqlx::query(
            "UPDATE relation_versions rv SET valid_to_key = u.valid_to_key \
             FROM UNNEST($1::uuid[], $2::bigint[]) AS u(id, valid_to_key) WHERE rv.id = u.id",
        )
        .bind(&ids)
        .bind(&valid_tos)
        .execute(&mut *tx)
        .await?;
    }
    if !sync_live_relation.is_empty() {
        let (rel_ids, source_ids): (Vec<Uuid>, Vec<Uuid>) = sync_live_relation.into_iter().unzip();
        sqlx::query(
            "INSERT INTO relations (id, entity_id, type_id, from_entity_id, from_space_id, \
             to_entity_id, to_space_id, position, space_id, verified) \
             SELECT u.relation_id, rv.entity_id, rv.type_id, rv.from_entity_id, rv.from_space_id, \
             rv.to_entity_id, rv.to_space_id, rv.position, rv.space_id, rv.verified \
             FROM UNNEST($1::uuid[], $2::uuid[]) AS u(relation_id, source_id) \
             JOIN relation_versions rv ON rv.id = u.source_id \
             ON CONFLICT (id) DO UPDATE SET entity_id = EXCLUDED.entity_id, type_id = EXCLUDED.type_id, \
             from_entity_id = EXCLUDED.from_entity_id, from_space_id = EXCLUDED.from_space_id, \
             to_entity_id = EXCLUDED.to_entity_id, to_space_id = EXCLUDED.to_space_id, \
             position = EXCLUDED.position, space_id = EXCLUDED.space_id, verified = EXCLUDED.verified",
        )
        .bind(&rel_ids)
        .bind(&source_ids)
        .execute(&mut *tx)
        .await?;
    }
    if !delete_live_relation_ids.is_empty() {
        sqlx::query("DELETE FROM relations WHERE id = ANY($1::uuid[])")
            .bind(&delete_live_relation_ids)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;
    println!("\nExecuted successfully.");

    Ok(())
}
