//! Per-block recompute orchestration.
//!
//! For each block an edit may have affected, recompute its aggregate in full
//! (design §7): `dedup -> eligibility -> scoring -> publish`. A full recompute
//! is always correct regardless of edit arrival order.
//!
//! All four stages are wired: dedup -> eligibility -> scoring -> publish.

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use crate::dedup::dedup_latest;
use crate::detect::DetectedEdit;
use crate::eligibility::{filter_eligible, SpaceKind};
use crate::error::IndexerError;
use crate::models::{BlockMeta, Ranking, RankingItem};
use crate::storage::Storage;
use crate::{publish, scoring};

/// Block ids whose aggregate may have changed as a result of this edit:
/// blocks whose own settings changed, blocks newly linked from a rank, and the
/// (possibly previously-linked) block of any rank/item touched here.
pub async fn affected_blocks(
    detected: &DetectedEdit,
    storage: &Storage,
) -> Result<HashSet<Uuid>, IndexerError> {
    let mut blocks: HashSet<Uuid> = HashSet::new();

    for b in &detected.blocks {
        blocks.insert(b.id);
    }
    for (_ranking_id, block_id) in &detected.block_links {
        blocks.insert(*block_id);
    }

    // Rankings/items touched here may already be linked to a block from a prior
    // edit — resolve their current block_id from the working tables.
    let mut ranking_ids: HashSet<Uuid> = HashSet::new();
    for r in &detected.rankings {
        ranking_ids.insert(r.id);
    }
    for it in &detected.items {
        ranking_ids.insert(it.ranking_id);
    }
    for ranking_id in ranking_ids {
        if let Some(block_id) = storage.block_id_for_ranking(ranking_id).await? {
            blocks.insert(block_id);
        }
    }

    Ok(blocks)
}

/// Recompute a single block's aggregate end to end.
pub async fn recompute_block(
    block_id: Uuid,
    meta: BlockMeta,
    storage: &Storage,
) -> Result<(), IndexerError> {
    let block = match storage.get_ranking_block(block_id).await? {
        Some(block) => block,
        None => {
            // The block's `TYPES -> Ranking Block` relation and its config can
            // arrive in separate edits, so detect() never registered it and the
            // rank would be silently never scored (issue #738). Recover the
            // block from the indexed graph and register it. If the entity isn't
            // a Ranking Block there (e.g. a rank linking to a not-yet-indexed
            // block), there is genuinely nothing to recompute yet.
            match storage.get_block_config_from_kg(block_id).await? {
                Some(block) => {
                    storage.upsert_ranking_block(&block).await?;
                    block
                }
                None => return Ok(()),
            }
        }
    };

    // 1. Dedup: keep the latest submission per (block, personal space).
    let submissions = storage.get_rankings_for_block(block_id).await?;
    let deduped = dedup_latest(submissions);

    // 2. Eligibility: resolve the block's space kind + member/editor set.
    let space_kind = storage
        .space_kind(block.space_id)
        .await?
        .unwrap_or(SpaceKind::Dao); // unknown space -> conservative (membership-restricted)
    let eligible_member_spaces = match space_kind {
        SpaceKind::Dao => storage.member_and_editor_spaces(block.space_id).await?,
        SpaceKind::Personal => HashSet::new(), // unused under "All of Geo"
    };
    let eligible = filter_eligible(&block, space_kind, &eligible_member_spaces, deduped);

    // 3. Scoring: normalize each ballot to [0.5, 1] and aggregate per (entity, space).
    let eligible_ids: Vec<Uuid> = eligible.iter().map(|r| r.id).collect();
    let items = storage.get_items_for_rankings(&eligible_ids).await?;
    let mut items_by_ranking: HashMap<Uuid, Vec<RankingItem>> = HashMap::new();
    for item in items {
        items_by_ranking
            .entry(item.ranking_id)
            .or_default()
            .push(item);
    }
    let ballots: Vec<(&Ranking, Vec<RankingItem>)> = eligible
        .iter()
        .map(|r| (r, items_by_ranking.remove(&r.id).unwrap_or_default()))
        .collect();
    let scores = scoring::aggregate(&ballots, scoring::NORM_LO, scoring::NORM_HI);
    storage.replace_ranking_scores(block_id, &scores).await?;

    // 4. Publish: project RANK_POSITION relations (+ the integer rank-position
    //    value on each reified entity) and Aggregated rankings provenance into
    //    the public graph, replacing the prior projection atomically.
    let projection = publish::build_projection(block_id, &scores);
    let contributing: Vec<Uuid> = ballots.iter().map(|(r, _)| r.id).collect();
    storage
        .replace_rank_position_projection(
            block_id,
            block.space_id,
            meta,
            &projection,
            &contributing,
        )
        .await?;

    tracing::debug!(
        block_id = %block_id,
        eligible_submissions = ballots.len(),
        ranked_entities = scores.len(),
        "ranking-indexer recompute: scored + published"
    );

    Ok(())
}

/// Apply a detected edit end to end: upsert its rank ops into the working
/// tables, then recompute every block it may have affected. Shared by the main
/// consumer loop and integration tests.
pub async fn apply_detected_edit(
    detected: &DetectedEdit,
    space_id: Uuid,
    storage: &Storage,
) -> Result<(), IndexerError> {
    if detected.is_empty() {
        return Ok(());
    }

    for block in &detected.blocks {
        storage.upsert_ranking_block(block).await?;
    }
    for ranking in &detected.rankings {
        storage.upsert_ranking(ranking).await?;
    }

    // Re-submission rebuilds a rank's items, so replace per ranking.
    let mut items_by_ranking: HashMap<Uuid, Vec<RankingItem>> = HashMap::new();
    for item in &detected.items {
        items_by_ranking
            .entry(item.ranking_id)
            .or_default()
            .push(item.clone());
    }
    for (ranking_id, items) in &items_by_ranking {
        storage.replace_ranking_items(*ranking_id, items).await?;
    }

    for (ranking_id, block_id) in &detected.block_links {
        // The RANK_BLOCK relation is authored in the ranking's space, so the
        // edit's `space_id` is the rank row's space.
        storage
            .set_ranking_block(*ranking_id, *block_id, space_id)
            .await?;
    }

    for block_id in affected_blocks(detected, storage).await? {
        recompute_block(block_id, detected.meta, storage).await?;
    }

    Ok(())
}
