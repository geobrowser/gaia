//! Fixes the 2026-07-21 space-misattribution incident: the batch-replay tool
//! (`replay_edit`) was called with the wrong `--space` for edits that had a
//! governing proposal — it used `ipfs_cache.space` (populated from the
//! on-chain action's `from_id`, i.e. the *proposer's* space) instead of the
//! proposal's actual `space_id` (`to_id`, the space the proposal executed
//! in). See the incident report for full context; see
//! `hermes-pipeline/src/pipelines/governance.rs` for the correct
//! attribution the real pipeline uses.
//!
//! For each misattributed edit this:
//! 1. Re-decodes the edit and re-derives exactly which (entity, property,
//!    space) / (relation, space) targets it touched, in the *wrong* space —
//!    same extraction `replay_edit` itself used.
//! 2. For each target, finds the row(s) the original bad replay wrote there:
//!    - a row it *opened* (`valid_from_key == this edit's version_key`) —
//!      exists only for Set/Create ops.
//!    - a row it *closed* (`valid_to_key == this edit's version_key`) —
//!      exists for any op type that touched a target with prior history in
//!      the wrong space, since closing happens unconditionally.
//! 3. Removes the opened row (if any) and un-closes / re-links the closed
//!    row (if any), bridging the wrong space's version chain around the
//!    removed node (same invariant `audit_batch_replay` checks).
//! 4. Reconciles the live `values`/`relations` row for each target against
//!    whatever is now open in the wrong space — including the case where
//!    `insert_edit_version` no-op'd (edit_id pre-existed from *before* the
//!    incident, e.g. already correctly indexed once) but the unconditional
//!    `insert_values`/`insert_relations` call still ran, leaving an orphan
//!    live row backed by zero version history. For relations specifically,
//!    `insert_relations`' `ON CONFLICT` never touches `space_id`, so a
//!    relation whose *current* space_id isn't the wrong one was already
//!    protected by an earlier legitimate write and is left untouched.
//! 5. Deletes the `edit_versions` row for this edit, so `replay_edit` can
//!    cleanly re-insert it under the correct space afterward.
//!
//! Performance: all data fetching is batched into a handful of bulk queries
//! (one pass to decode every edit and collect its targets, then one query
//! per lookup type across *all* targets at once) rather than several
//! queries per individual target — the latter took hours across a few
//! hundred edits with many targets each.
//!
//! Defaults to `--dry-run`. Pass `--execute` to actually apply. This tool
//! does NOT re-replay into the correct space — run `replay_edit` for each
//! entry afterward, in block order.
//!
//! Usage:
//!   DATABASE_URL=... cargo run -p kg-indexer --bin fix_misattribution -- \
//!     --batch-file /path/to/uri_block_wrongspace_correctspace_lines.txt \
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

struct BatchLine {
    uri: String,
    block: u64,
    wrong_space: Uuid,
    #[allow(dead_code)]
    correct_space: Uuid,
}

struct EditPlan {
    uri: String,
    wrong_space: Uuid,
    version_key: i64,
    edit_id: Uuid,
    edit_name: String,
    value_targets: Vec<(Uuid, Uuid)>,
    relation_targets: Vec<Uuid>,
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

    let content = fs::read_to_string(batch_file)
        .map_err(|e| IndexerError::Config(format!("could not read batch file: {e}")))?;

    let mut lines = Vec::new();
    for line in content.lines().filter(|l| !l.trim().is_empty()) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() != 4 {
            return Err(IndexerError::Config(format!(
                "malformed line (expected \"<uri> <block> <wrong_space> <correct_space>\"): {line:?}"
            )));
        }
        let wrong_space: Uuid = parts[2]
            .parse()
            .map_err(|e| IndexerError::Config(format!("bad wrong_space in line {line:?}: {e}")))?;
        let correct_space: Uuid = parts[3].parse().map_err(|e| {
            IndexerError::Config(format!("bad correct_space in line {line:?}: {e}"))
        })?;
        if wrong_space == correct_space {
            return Err(IndexerError::Config(format!(
                "wrong_space == correct_space in line {line:?} — this tool deletes/relinks \
                 history in wrong_space, so a malformed line here would corrupt the correct \
                 space instead of fixing it"
            )));
        }
        lines.push(BatchLine {
            uri: parts[0].to_string(),
            block: parts[1]
                .parse()
                .map_err(|e| IndexerError::Config(format!("bad block in line {line:?}: {e}")))?,
            wrong_space,
            correct_space,
        });
    }
    lines.sort_by_key(|l| l.block);

    println!(
        "{} edit(s) to fix. Mode: {}",
        lines.len(),
        if execute { "EXECUTE" } else { "DRY RUN" }
    );

    // ---- Phase 1: fetch + decode every edit, collect its targets ----
    let uris: Vec<&str> = lines.iter().map(|l| l.uri.as_str()).collect();
    let cache_rows: Vec<(String, Option<Vec<u8>>, bool)> =
        sqlx::query_as("SELECT uri, data, is_errored FROM ipfs_cache WHERE uri = ANY($1::text[])")
            .bind(&uris)
            .fetch_all(&storage.pool)
            .await?;
    let cache_by_uri: HashMap<String, (Option<Vec<u8>>, bool)> = cache_rows
        .into_iter()
        .map(|(uri, data, errored)| (uri, (data, errored)))
        .collect();

    let mut plans: Vec<EditPlan> = Vec::with_capacity(lines.len());
    let mut skipped = 0u32;

    for line in &lines {
        match build_plan(line, &cache_by_uri) {
            Ok(plan) => plans.push(plan),
            Err(e) => {
                println!("SKIP {}: {e}", line.uri);
                skipped += 1;
            }
        }
    }

    // The original bad replay's version_key is whatever `replay_edit` was
    // actually invoked with — `(block << 32) | sequence`, not necessarily
    // `sequence = 0`. Rather than assume, read it back from each edit's
    // still-present (wrong-space) `edit_versions` row, which is exactly the
    // value the opened/closed rows in value_versions/relation_versions were
    // written with.
    let plan_edit_ids: Vec<Uuid> = plans.iter().map(|p| p.edit_id).collect();
    let edit_version_keys: Vec<(Uuid, i64)> = if plan_edit_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as(
            "SELECT edit_id, version_key FROM edit_versions WHERE edit_id = ANY($1::uuid[])",
        )
        .bind(&plan_edit_ids)
        .fetch_all(&storage.pool)
        .await?
    };
    let version_key_by_edit_id: HashMap<Uuid, i64> = edit_version_keys.into_iter().collect();

    let mut resolved_plans = Vec::with_capacity(plans.len());
    for mut plan in plans {
        match version_key_by_edit_id.get(&plan.edit_id) {
            Some(&version_key) => {
                plan.version_key = version_key;
                resolved_plans.push(plan);
            }
            None => {
                println!(
                    "SKIP {}: no edit_versions row for edit_id {} — already cleaned up, or never replayed?",
                    plan.uri, plan.edit_id
                );
                skipped += 1;
            }
        }
    }
    let plans = resolved_plans;

    println!(
        "Decoded {} edit(s), {} skipped. Collecting targets...",
        plans.len(),
        skipped
    );

    // ---- Phase 2: flatten all targets across all edits ----
    struct ValueTouch {
        plan_idx: usize,
        entity_id: Uuid,
        property_id: Uuid,
        wrong_space: Uuid,
        version_key: i64,
        opened_id: Uuid,
        live_id: String,
    }
    struct RelationTouch {
        plan_idx: usize,
        relation_id: Uuid,
        wrong_space: Uuid,
        version_key: i64,
        opened_id: Uuid,
    }

    let mut value_touches = Vec::new();
    let mut relation_touches = Vec::new();

    for (plan_idx, plan) in plans.iter().enumerate() {
        for &(entity_id, property_id) in &plan.value_targets {
            let opened_id = Storage::derive_value_version_id(
                &entity_id,
                &property_id,
                &plan.wrong_space,
                plan.version_key,
            );
            let live_id =
                handlers::edits::derive_value_id(&entity_id, &property_id, &plan.wrong_space)
                    .to_string();
            value_touches.push(ValueTouch {
                plan_idx,
                entity_id,
                property_id,
                wrong_space: plan.wrong_space,
                version_key: plan.version_key,
                opened_id,
                live_id,
            });
        }
        for &relation_id in &plan.relation_targets {
            let opened_id = Storage::derive_relation_version_id(
                &relation_id,
                &plan.wrong_space,
                plan.version_key,
            );
            relation_touches.push(RelationTouch {
                plan_idx,
                relation_id,
                wrong_space: plan.wrong_space,
                version_key: plan.version_key,
                opened_id,
            });
        }
    }

    println!(
        "{} value target(s), {} relation target(s) across all edits. Batch-fetching current state...",
        value_touches.len(),
        relation_touches.len()
    );

    // ---- Phase 3: batch-fetch everything needed, in a handful of queries ----
    let opened_value_ids: Vec<Uuid> = value_touches.iter().map(|t| t.opened_id).collect();
    let opened_values: Vec<(Uuid, i64, Option<i64>)> = sqlx::query_as(
        "SELECT id, valid_from_key, valid_to_key FROM value_versions WHERE id = ANY($1::uuid[])",
    )
    .bind(&opened_value_ids)
    .fetch_all(&storage.pool)
    .await?;
    let opened_value_map: HashMap<Uuid, (i64, Option<i64>)> = opened_values
        .into_iter()
        .map(|(id, from, to)| (id, (from, to)))
        .collect();

    let (ve, vp, vs, vk): (Vec<Uuid>, Vec<Uuid>, Vec<Uuid>, Vec<i64>) = {
        let mut ve = Vec::new();
        let mut vp = Vec::new();
        let mut vs = Vec::new();
        let mut vk = Vec::new();
        for t in &value_touches {
            ve.push(t.entity_id);
            vp.push(t.property_id);
            vs.push(t.wrong_space);
            vk.push(t.version_key);
        }
        (ve, vp, vs, vk)
    };
    let closed_values: Vec<(Uuid, Uuid, Uuid, i64, Uuid, i64)> = sqlx::query_as(
        "SELECT vv.entity_id, vv.property_id, vv.space_id, u.version_key, vv.id, vv.valid_from_key \
         FROM UNNEST($1::uuid[], $2::uuid[], $3::uuid[], $4::bigint[]) AS u(entity_id, property_id, space_id, version_key) \
         JOIN value_versions vv ON vv.entity_id = u.entity_id AND vv.property_id = u.property_id \
           AND vv.space_id = u.space_id AND vv.valid_to_key = u.version_key",
    )
    .bind(&ve)
    .bind(&vp)
    .bind(&vs)
    .bind(&vk)
    .fetch_all(&storage.pool)
    .await?;
    let closed_value_map: HashMap<(Uuid, Uuid, Uuid, i64), (Uuid, i64)> = closed_values
        .into_iter()
        .map(|(e, p, s, k, id, from)| ((e, p, s, k), (id, from)))
        .collect();

    let distinct_value_targets: Vec<(Uuid, Uuid, Uuid)> = {
        let mut seen = HashSet::new();
        value_touches
            .iter()
            .filter_map(|t| {
                let key = (t.entity_id, t.property_id, t.wrong_space);
                seen.insert(key).then_some(key)
            })
            .collect()
    };
    // `now_open` state (which row is currently the latest for each target)
    // is deliberately NOT fetched here. It's read fresh after the
    // version-table mutations run (see Phase 5) — a target's "now open" row
    // can be exactly the row this tool is about to delete, and syncing the
    // live row to a snapshot taken before that delete would point it at a
    // row that no longer exists by the time the sync statement runs.

    let live_value_ids: Vec<String> = distinct_value_targets
        .iter()
        .map(|&(e, p, s)| handlers::edits::derive_value_id(&e, &p, &s).to_string())
        .collect();
    let live_values: Vec<String> =
        sqlx::query_scalar("SELECT id FROM values WHERE id = ANY($1::text[])")
            .bind(&live_value_ids)
            .fetch_all(&storage.pool)
            .await?;
    let live_value_set: HashSet<String> = live_values.into_iter().collect();

    // Relations
    let opened_relation_ids: Vec<Uuid> = relation_touches.iter().map(|t| t.opened_id).collect();
    let opened_relations: Vec<(Uuid, i64, Option<i64>)> = sqlx::query_as(
        "SELECT id, valid_from_key, valid_to_key FROM relation_versions WHERE id = ANY($1::uuid[])",
    )
    .bind(&opened_relation_ids)
    .fetch_all(&storage.pool)
    .await?;
    let opened_relation_map: HashMap<Uuid, (i64, Option<i64>)> = opened_relations
        .into_iter()
        .map(|(id, from, to)| (id, (from, to)))
        .collect();

    let (rr, rs, rk): (Vec<Uuid>, Vec<Uuid>, Vec<i64>) = {
        let mut rr = Vec::new();
        let mut rs = Vec::new();
        let mut rk = Vec::new();
        for t in &relation_touches {
            rr.push(t.relation_id);
            rs.push(t.wrong_space);
            rk.push(t.version_key);
        }
        (rr, rs, rk)
    };
    let closed_relations: Vec<(Uuid, Uuid, i64, Uuid, i64)> = sqlx::query_as(
        "SELECT rv.relation_id, u.space_id, u.version_key, rv.id, rv.valid_from_key \
         FROM UNNEST($1::uuid[], $2::uuid[], $3::bigint[]) AS u(relation_id, space_id, version_key) \
         JOIN relation_versions rv ON rv.relation_id = u.relation_id AND rv.space_id = u.space_id \
           AND rv.valid_to_key = u.version_key",
    )
    .bind(&rr)
    .bind(&rs)
    .bind(&rk)
    .fetch_all(&storage.pool)
    .await?;
    let closed_relation_map: HashMap<(Uuid, Uuid, i64), (Uuid, i64)> = closed_relations
        .into_iter()
        .map(|(r, s, k, id, from)| ((r, s, k), (id, from)))
        .collect();

    let distinct_relation_targets: Vec<(Uuid, Uuid)> = {
        let mut seen = HashSet::new();
        relation_touches
            .iter()
            .filter_map(|t| {
                let key = (t.relation_id, t.wrong_space);
                seen.insert(key).then_some(key)
            })
            .collect()
    };
    let relation_ids: Vec<Uuid> = distinct_relation_targets.iter().map(|&(r, _)| r).collect();
    let current_relations: Vec<(Uuid, Uuid)> =
        sqlx::query_as("SELECT id, space_id FROM relations WHERE id = ANY($1::uuid[])")
            .bind(&relation_ids)
            .fetch_all(&storage.pool)
            .await?;
    let current_relation_space_map: HashMap<Uuid, Uuid> = current_relations.into_iter().collect();

    println!("Batch fetch complete. Computing plan...");

    // ---- Phase 4: compute version-table mutations in memory ----
    let mut delete_value_version_ids: Vec<Uuid> = Vec::new();
    let mut update_value_version_valid_to: Vec<(Uuid, Option<i64>)> = Vec::new();
    let mut delete_relation_version_ids: Vec<Uuid> = Vec::new();
    let mut update_relation_version_valid_to: Vec<(Uuid, Option<i64>)> = Vec::new();

    // Whether THIS run found any actual wrong-space version-table footprint
    // for each plan. An edit whose every target shows "no trace" likely
    // pre-existed from before the incident — `insert_edit_version` no-op'd
    // (ON CONFLICT DO NOTHING) so its edit_versions row is from whatever
    // legitimately indexed it earlier, not from this bad replay. Deleting
    // that row would remove replay_edit's idempotency guard for an edit
    // that was never actually broken, and re-processing it would re-close
    // currently-open versions and reintroduce backfill-style corruption.
    // Edits with zero targets at all have nothing to find a trace of, so
    // there's nothing at risk — treat them as safe to clear.
    let mut has_footprint: Vec<bool> = plans
        .iter()
        .map(|p| p.value_targets.is_empty() && p.relation_targets.is_empty())
        .collect();

    for t in &value_touches {
        let opened = opened_value_map.get(&t.opened_id).copied();
        let closed = closed_value_map
            .get(&(t.entity_id, t.property_id, t.wrong_space, t.version_key))
            .copied();

        if opened.is_none() && closed.is_none() {
            println!(
                "  [edit {}] VALUE (entity={}, property={}): no value_versions trace at version_key={} \
                 (edit_id likely pre-existed from before the incident)",
                plans[t.plan_idx].uri, t.entity_id, t.property_id, t.version_key
            );
        } else {
            has_footprint[t.plan_idx] = true;
            let new_valid_to = opened.and_then(|(_, to)| to);
            if let Some((closed_id, closed_from)) = closed {
                println!(
                    "  [edit {}] VALUE (entity={}, property={}): un-close row {closed_id} (valid_from={closed_from}) -> new valid_to={new_valid_to:?}",
                    plans[t.plan_idx].uri, t.entity_id, t.property_id
                );
                update_value_version_valid_to.push((closed_id, new_valid_to));
            }
            if opened.is_some() {
                println!(
                    "  [edit {}] VALUE (entity={}, property={}): delete opened row {}",
                    plans[t.plan_idx].uri, t.entity_id, t.property_id, t.opened_id
                );
                delete_value_version_ids.push(t.opened_id);
            }
        }
    }

    for t in &relation_touches {
        let opened = opened_relation_map.get(&t.opened_id).copied();
        let closed = closed_relation_map
            .get(&(t.relation_id, t.wrong_space, t.version_key))
            .copied();

        if opened.is_none() && closed.is_none() {
            println!(
                "  [edit {}] RELATION (id={}): no relation_versions trace at version_key={}",
                plans[t.plan_idx].uri, t.relation_id, t.version_key
            );
        } else {
            has_footprint[t.plan_idx] = true;
            let new_valid_to = opened.and_then(|(_, to)| to);
            if let Some((closed_id, closed_from)) = closed {
                println!(
                    "  [edit {}] RELATION (id={}): un-close row {closed_id} (valid_from={closed_from}) -> new valid_to={new_valid_to:?}",
                    plans[t.plan_idx].uri, t.relation_id
                );
                update_relation_version_valid_to.push((closed_id, new_valid_to));
            }
            if opened.is_some() {
                println!(
                    "  [edit {}] RELATION (id={}): delete opened row {}",
                    plans[t.plan_idx].uri, t.relation_id, t.opened_id
                );
                delete_relation_version_ids.push(t.opened_id);
            }
        }
    }

    let mut edit_ids: Vec<Uuid> = Vec::new();
    for (plan_idx, plan) in plans.iter().enumerate() {
        if has_footprint[plan_idx] {
            edit_ids.push(plan.edit_id);
            println!(
                "  [edit {}] WOULD DELETE edit_versions WHERE edit_id = {} ({:?})",
                plan.uri, plan.edit_id, plan.edit_name
            );
        } else {
            println!(
                "  [edit {}] SKIP edit_versions delete for edit_id = {} ({:?}) — no wrong-space \
                 footprint found for any of its targets, so it likely pre-existed from before the \
                 incident; deleting would remove replay_edit's idempotency guard for an edit that \
                 was never actually broken",
                plan.uri, plan.edit_id, plan.edit_name
            );
        }
    }

    // Capture counts before these vecs are consumed below.
    let delete_value_version_count = delete_value_version_ids.len();
    let update_value_version_count = update_value_version_valid_to.len();
    let delete_relation_version_count = delete_relation_version_ids.len();
    let update_relation_version_count = update_relation_version_valid_to.len();

    // ---- Phase 5: mutate version tables, THEN decide live-row sync from
    // the post-mutation state. Always runs inside a transaction — a
    // target's "now open" row can be exactly the row we just deleted, so
    // computing that from a pre-mutation snapshot (like Phase 3's queries
    // would if reused here) risks syncing a live row to something that no
    // longer exists by the time the sync statement runs. Dry-run rolls the
    // transaction back at the end, so the printed plan is always exactly
    // what --execute would do.
    let mut tx = storage.pool.begin().await?;

    if !delete_value_version_ids.is_empty() {
        sqlx::query("DELETE FROM value_versions WHERE id = ANY($1::uuid[])")
            .bind(&delete_value_version_ids)
            .execute(&mut *tx)
            .await?;
    }
    if !update_value_version_valid_to.is_empty() {
        let (ids, valid_tos): (Vec<Uuid>, Vec<Option<i64>>) =
            update_value_version_valid_to.into_iter().unzip();
        sqlx::query(
            "UPDATE value_versions vv SET valid_to_key = u.valid_to_key \
             FROM UNNEST($1::uuid[], $2::bigint[]) AS u(id, valid_to_key) WHERE vv.id = u.id",
        )
        .bind(&ids)
        .bind(&valid_tos)
        .execute(&mut *tx)
        .await?;
    }
    if !delete_relation_version_ids.is_empty() {
        sqlx::query("DELETE FROM relation_versions WHERE id = ANY($1::uuid[])")
            .bind(&delete_relation_version_ids)
            .execute(&mut *tx)
            .await?;
    }
    if !update_relation_version_valid_to.is_empty() {
        let (ids, valid_tos): (Vec<Uuid>, Vec<Option<i64>>) =
            update_relation_version_valid_to.into_iter().unzip();
        sqlx::query(
            "UPDATE relation_versions rv SET valid_to_key = u.valid_to_key \
             FROM UNNEST($1::uuid[], $2::bigint[]) AS u(id, valid_to_key) WHERE rv.id = u.id",
        )
        .bind(&ids)
        .bind(&valid_tos)
        .execute(&mut *tx)
        .await?;
    }

    // Fresh post-mutation "now open" lookups, read inside the same
    // transaction (READ COMMITTED sees this transaction's own prior writes).
    let (nve, nvp, nvs): (Vec<Uuid>, Vec<Uuid>, Vec<Uuid>) = distinct_value_targets
        .iter()
        .map(|&(e, p, s)| (e, p, s))
        .fold(
            (Vec::new(), Vec::new(), Vec::new()),
            |mut acc, (e, p, s)| {
                acc.0.push(e);
                acc.1.push(p);
                acc.2.push(s);
                acc
            },
        );
    let now_open_values: Vec<(Uuid, Uuid, Uuid, Uuid)> = sqlx::query_as(
        "SELECT entity_id, property_id, space_id, id FROM value_versions \
         WHERE (entity_id, property_id, space_id) IN (\
           SELECT * FROM UNNEST($1::uuid[], $2::uuid[], $3::uuid[])\
         ) AND valid_to_key IS NULL",
    )
    .bind(&nve)
    .bind(&nvp)
    .bind(&nvs)
    .fetch_all(&mut *tx)
    .await?;
    let now_open_value_map: HashMap<(Uuid, Uuid, Uuid), Uuid> = now_open_values
        .into_iter()
        .map(|(e, p, s, id)| ((e, p, s), id))
        .collect();

    let (nrr, nrs): (Vec<Uuid>, Vec<Uuid>) = distinct_relation_targets
        .iter()
        .map(|&(r, s)| (r, s))
        .fold((Vec::new(), Vec::new()), |mut acc, (r, s)| {
            acc.0.push(r);
            acc.1.push(s);
            acc
        });
    let now_open_relations: Vec<(Uuid, Uuid, Uuid)> = sqlx::query_as(
        "SELECT relation_id, space_id, id FROM relation_versions \
         WHERE (relation_id, space_id) IN (SELECT * FROM UNNEST($1::uuid[], $2::uuid[])) \
         AND valid_to_key IS NULL",
    )
    .bind(&nrr)
    .bind(&nrs)
    .fetch_all(&mut *tx)
    .await?;
    let now_open_relation_map: HashMap<(Uuid, Uuid), Uuid> = now_open_relations
        .into_iter()
        .map(|(r, s, id)| ((r, s), id))
        .collect();

    let mut sync_live_value: Vec<(String, Uuid)> = Vec::new(); // (live_id, source_version_id)
    let mut delete_live_value_ids: Vec<String> = Vec::new();
    let mut seen_value_targets: HashSet<(Uuid, Uuid, Uuid)> = HashSet::new();

    for t in &value_touches {
        let key = (t.entity_id, t.property_id, t.wrong_space);
        if !seen_value_targets.insert(key) {
            continue; // already decided for this target by an earlier touch
        }
        let now_open = now_open_value_map.get(&key).copied();
        let live_exists = live_value_set.contains(&t.live_id);
        match (now_open, live_exists) {
            (Some(open_id), _) => {
                println!(
                    "  [edit {}] VALUE live sync: set values id={} to match now-open row {open_id}",
                    plans[t.plan_idx].uri, t.live_id
                );
                sync_live_value.push((t.live_id.clone(), open_id));
            }
            (None, true) => {
                println!("  [edit {}] VALUE live sync: delete orphaned values id={} (no version history backs it)", plans[t.plan_idx].uri, t.live_id);
                delete_live_value_ids.push(t.live_id.clone());
            }
            (None, false) => {}
        }
    }

    let mut sync_live_relation: Vec<(Uuid, Uuid)> = Vec::new(); // (relation_id, source_version_id)
    let mut delete_live_relation_ids: Vec<Uuid> = Vec::new();
    let mut seen_relation_targets: HashSet<(Uuid, Uuid)> = HashSet::new();

    for t in &relation_touches {
        let key = (t.relation_id, t.wrong_space);
        if !seen_relation_targets.insert(key) {
            continue;
        }
        let current_space = current_relation_space_map.get(&t.relation_id).copied();
        if current_space != Some(t.wrong_space) {
            println!(
                "  [edit {}] RELATION live sync: not needed (relations id={} current space_id={:?} is not the wrong space)",
                plans[t.plan_idx].uri, t.relation_id, current_space
            );
            continue;
        }
        match now_open_relation_map.get(&key).copied() {
            Some(open_id) => {
                println!("  [edit {}] RELATION live sync: set relations id={} to match now-open row {open_id}", plans[t.plan_idx].uri, t.relation_id);
                sync_live_relation.push((t.relation_id, open_id));
            }
            None => {
                println!("  [edit {}] RELATION live sync: delete relations id={} (currently wrong-space, nothing precedes it)", plans[t.plan_idx].uri, t.relation_id);
                delete_live_relation_ids.push(t.relation_id);
            }
        }
    }

    println!(
        "\nPlan: delete {} value_version row(s), update {} value_version row(s), sync {} live value row(s), \
         delete {} orphaned live value row(s); delete {} relation_version row(s), update {} relation_version row(s), \
         sync {} live relation row(s), delete {} live relation row(s); delete {} edit_versions row(s).",
        delete_value_version_count,
        update_value_version_count,
        sync_live_value.len(),
        delete_live_value_ids.len(),
        delete_relation_version_count,
        update_relation_version_count,
        sync_live_relation.len(),
        delete_live_relation_ids.len(),
        edit_ids.len(),
    );

    if !execute {
        tx.rollback().await?;
        println!("\nDry run — no changes made. Pass --execute to apply.");
        return Ok(());
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
    if !sync_live_relation.is_empty() {
        let (rel_ids, source_ids): (Vec<Uuid>, Vec<Uuid>) = sync_live_relation.into_iter().unzip();
        sqlx::query(
            "UPDATE relations r SET entity_id = rv.entity_id, type_id = rv.type_id, \
             from_entity_id = rv.from_entity_id, from_space_id = rv.from_space_id, \
             to_entity_id = rv.to_entity_id, to_space_id = rv.to_space_id, position = rv.position, \
             space_id = rv.space_id, verified = rv.verified \
             FROM UNNEST($1::uuid[], $2::uuid[]) AS u(relation_id, source_id) \
             JOIN relation_versions rv ON rv.id = u.source_id \
             WHERE r.id = u.relation_id AND r.is_system = false",
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

    sqlx::query("DELETE FROM edit_versions WHERE edit_id = ANY($1::uuid[])")
        .bind(&edit_ids)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    println!("\nExecuted successfully.");

    Ok(())
}

fn build_plan(
    line: &BatchLine,
    cache_by_uri: &HashMap<String, (Option<Vec<u8>>, bool)>,
) -> Result<EditPlan, IndexerError> {
    let (data, is_errored) = cache_by_uri.get(&line.uri).ok_or_else(|| {
        IndexerError::Config(format!("uri not found in ipfs_cache: {}", line.uri))
    })?;
    if *is_errored {
        return Err(IndexerError::Config("ipfs_cache row is_errored".into()));
    }
    let payload = data
        .clone()
        .ok_or_else(|| IndexerError::Config("no data".into()))?;
    let decoded =
        decode_edit(&payload).map_err(|e| IndexerError::Config(format!("decode failed: {e}")))?;

    let edit = HermesEdit {
        id: decoded.id.to_vec(),
        name: decoded.name.to_string(),
        payload: payload.clone(),
        authors: decoded.authors.iter().map(|a| a.to_vec()).collect(),
        language: None,
        space_id: line.wrong_space.as_bytes().to_vec(),
        is_canonical: true,
        meta: Some(BlockchainMetadata {
            created_at: 0,
            created_by: vec![],
            block_number: line.block,
            cursor: String::new(),
            sequence: 0,
            is_last: false,
        }),
    };

    let result = handlers::edits::handle_edit(&edit)?;

    let mut value_targets: Vec<(Uuid, Uuid)> = Vec::new();
    for v in &result.values {
        let t = (v.entity_id, v.property_id);
        if !value_targets.contains(&t) {
            value_targets.push(t);
        }
    }
    let mut relation_targets: Vec<Uuid> = Vec::new();
    for r in &result.relations {
        let id = r.id();
        if !relation_targets.contains(&id) {
            relation_targets.push(id);
        }
    }

    Ok(EditPlan {
        uri: line.uri.clone(),
        wrong_space: line.wrong_space,
        // Placeholder — overwritten from the edit's actual `edit_versions`
        // row (see main()) once every plan's edit_id is known, since the
        // true version_key may include a non-zero sequence.
        version_key: 0,
        edit_id: result.edit_id,
        edit_name: decoded.name.to_string(),
        value_targets,
        relation_targets,
    })
}
