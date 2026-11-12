//! Per-block recompute orchestration.
//!
//! For each block an edit may have affected, recompute its aggregate in full
//! (design §7): `dedup -> eligibility -> scoring -> publish`. A full recompute
//! is always correct regardless of edit arrival order.
//!
//! All four stages are wired: dedup -> eligibility -> scoring -> publish.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
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
    for link in &detected.block_links {
        blocks.insert(link.block_id);
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

/// Pair each eligible ranking with its items, in `eligible`'s order.
///
/// A ranking with no items is not a ballot (GEO-3591): a ranker who cleared
/// their ranking contributes nothing, is not counted in an average-mode block's
/// ballot counts, and is not published as an `Aggregated rankings` source. The
/// caller passes rankings that are already deduped, so a cleared ranking still
/// supersedes the same ranker's older submission to the block.
pub fn ballots_with_items(
    eligible: &[Ranking],
    items: Vec<RankingItem>,
) -> Vec<(&Ranking, Vec<RankingItem>)> {
    let mut items_by_ranking: HashMap<Uuid, Vec<RankingItem>> = HashMap::new();
    for item in items {
        items_by_ranking
            .entry(item.ranking_id)
            .or_default()
            .push(item);
    }
    eligible
        .iter()
        .filter_map(|r| items_by_ranking.remove(&r.id).map(|items| (r, items)))
        .collect()
}

/// Recompute a single block's aggregate end to end.
///
/// `now` is the instant ages are measured against — a static block's
/// `[start, end]` window, and a Rolling block's recency decay
/// (`scoring::recency_weight`). It is a parameter, not an inline `Utc::now()`
/// call, so a test can recompute the same block twice with an advanced `now`
/// and watch an old ballot's weight fall without a real sleep. Live callers
/// (the edit consumer, membership events) pass `Utc::now()` at their own
/// boundary.
///
/// Note what `now` no longer does: it does not remove anything from a Rolling
/// block. Ballots are never aged out (GEO-2869); they are only weighted down.
pub async fn recompute_block(
    block_id: Uuid,
    meta: BlockMeta,
    now: DateTime<Utc>,
    storage: &Storage,
) -> Result<(), IndexerError> {
    // One recompute of a block at a time (the consumer and the hourly
    // backfill can both reach it). Released when `lock` commits below, or is
    // dropped on any early return or error.
    let mut lock = storage.lock_block_for_recompute(block_id).await?;
    // Queue entries for this block visible before we read its inputs
    // (GEO-3591). Exactly these are cleared below with the commit; an entry
    // committed after this read stays queued for the next recompute.
    let seen = storage.pending_recompute_seen(&mut lock, block_id).await?;

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
                None => {
                    // Nothing to score, so nothing left to do for a queued delete.
                    storage.clear_pending_recompute(&mut lock, &seen).await?;
                    lock.commit().await?;
                    return Ok(());
                }
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

    // 3. Scoring: normalize each ballot to [0.5, 1] and aggregate per (entity, space),
    //    or per entity for an average-mode block.
    let eligible_ids: Vec<Uuid> = eligible.iter().map(|r| r.id).collect();
    let items = storage.get_items_for_rankings(&eligible_ids).await?;
    let ballots = ballots_with_items(&eligible, items);
    // Rolling blocks weight each ballot by recency, using the block's
    // `submission_frequency` as the half-life; static blocks score every eligible
    // ballot at full weight.
    let decay = block
        .ranking_type
        .and(block.submission_frequency)
        .map(|frequency_hours| scoring::RecencyDecay {
            half_life_hours: f64::from(frequency_hours),
            now,
        });
    // The block's own row cap and scoring mode (GEO-3676); the default for a
    // block with no `ranks.ranking_block_config` row is 200 rows, summed.
    let config = storage.block_scoring(block_id).await?;
    let scores =
        scoring::aggregate_with(&ballots, scoring::NORM_LO, scoring::NORM_HI, decay, config);
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
    // Only once the projection is written: a config change is not applied
    // until the public graph shows it.
    storage.mark_block_scoring_applied(block_id, config).await?;

    storage.clear_pending_recompute(&mut lock, &seen).await?;
    lock.commit().await?;

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

    // Deletes first (GEO-3591), so a create of the same id later in the edit
    // wins, and `ranks.update`'s delete-every-old-vote + create-every-new-vote
    // ends with exactly the new ballot whatever order its ops came in. Each is
    // scoped to the edit's space, as the kg-indexer scopes its deletes: a
    // delete from any other space changes nothing.
    //
    // Each delete queues the blocks it changed in `ranks.pending_recomputes`
    // in the same statement and returns them. The returned blocks are this
    // edit's own and are recomputed below. A retry of this edit after a failed
    // recompute deletes nothing the second time; the queue still holds the
    // blocks, and the drain after the edit's own blocks picks them up.
    let mut own: HashSet<Uuid> = HashSet::new();
    if !detected.deleted_entities.is_empty() {
        own.extend(
            storage
                .delete_rankings(
                    &detected.deleted_entities,
                    &detected.deleted_ranks,
                    space_id,
                )
                .await?,
        );
    }
    if !detected.deleted_relations.is_empty() {
        own.extend(
            storage
                .unlink_rankings_by_relation(&detected.deleted_relations, space_id)
                .await?,
        );
        own.extend(
            storage
                .delete_ranking_items_by_relation(&detected.deleted_relations, space_id)
                .await?,
        );
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
        storage
            .replace_ranking_items(*ranking_id, items, space_id)
            .await?;
    }

    for link in &detected.block_links {
        // The RANK_BLOCK relation is authored in the ranking's space, so the
        // edit's `space_id` is the rank row's space.
        // A rank moving block also rescores the block it left.
        own.extend(
            storage
                .set_ranking_block(link.ranking_id, link.block_id, link.relation_id, space_id)
                .await?,
        );
    }

    // This edit's own blocks first, with errors propagating as for any edit.
    let now = Utc::now();
    own.extend(affected_blocks(detected, storage).await?);
    for block_id in &own {
        recompute_block(*block_id, detected.meta, now, storage).await?;
    }

    // Then whatever an earlier failed run left queued, best effort: a block
    // that fails here must not fail an unrelated edit, and one that keeps
    // failing is left to the hourly backfill after a few tries. This runs for
    // every edit that reaches here (any rank-relevant op), not only deletes:
    // a queue entry can come from any write that queues (a delete, or a rank
    // moving block), and a redelivered edit no longer sees what it queued
    // the first time.
    drain_pending_recomputes(
        storage,
        detected.meta,
        now,
        Some(LIVE_DRAIN_MAX_ATTEMPTS),
        &own,
    )
    .await;

    Ok(())
}

/// How many failed recomputes of a queued block the live consumer tries
/// before leaving it to the hourly backfill.
pub const LIVE_DRAIN_MAX_ATTEMPTS: i32 = 3;

/// Recompute the blocks in `ranks.pending_recomputes` (GEO-3591), except
/// `skip`. A failure is logged, counted in
/// `ranking_indexer_pending_recompute_failures_total` and in the entry's
/// `attempts`, and does not stop the others. `max_attempts` leaves out blocks
/// that have already failed that often (the live consumer); `None` takes every
/// queued block (the hourly backfill). Returns `(recomputed, failed)`.
pub async fn drain_pending_recomputes(
    storage: &Storage,
    meta: BlockMeta,
    now: DateTime<Utc>,
    max_attempts: Option<i32>,
    skip: &HashSet<Uuid>,
) -> (usize, usize) {
    let queued = match storage.pending_recompute_blocks(max_attempts).await {
        Ok(queued) => queued,
        Err(e) => {
            tracing::warn!(error = %e, "could not read the pending recompute queue");
            crate::metrics::pending_recompute_failed();
            return (0, 1);
        }
    };
    let (mut recomputed, mut failed) = (0, 0);
    for block_id in queued.into_iter().filter(|b| !skip.contains(b)) {
        match recompute_block(block_id, meta, now, storage).await {
            Ok(()) => recomputed += 1,
            Err(e) => {
                failed += 1;
                crate::metrics::pending_recompute_failed();
                tracing::warn!(
                    block_id = %block_id,
                    error = %e,
                    "queued block recompute failed; left queued"
                );
                if let Err(e) = storage.note_pending_recompute_failure(block_id).await {
                    tracing::warn!(block_id = %block_id, error = %e, "could not count the failure");
                }
            }
        }
    }
    (recomputed, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranking(id: u128) -> Ranking {
        Ranking {
            id: Uuid::from_u128(id),
            block_id: Some(Uuid::from_u128(1)),
            space_id: Uuid::from_u128(900 + id),
            author_address: None,
            rank_type: Some("ORDINAL".to_string()),
            submitted_at: None,
            updated_at_block: 0,
            update_index: 0,
            block_relation_id: None,
        }
    }

    fn item(ranking: u128, entity: u128) -> RankingItem {
        RankingItem {
            ranking_id: Uuid::from_u128(ranking),
            entity_id: Uuid::from_u128(entity),
            space_id: Uuid::from_u128(500),
            position: Some("a0".into()),
            weight: None,
            relation_id: None,
        }
    }

    #[test]
    fn a_ranking_with_no_items_is_not_a_ballot() {
        // Ranking 2 was cleared: every vote deleted, none created.
        let eligible = vec![ranking(1), ranking(2), ranking(3)];
        let items = vec![item(1, 100), item(3, 100), item(3, 101)];
        let ballots = ballots_with_items(&eligible, items);
        let ids: Vec<Uuid> = ballots.iter().map(|(r, _)| r.id).collect();
        assert_eq!(ids, vec![Uuid::from_u128(1), Uuid::from_u128(3)]);
        assert_eq!(ballots[1].1.len(), 2);
    }

    #[test]
    fn a_cleared_ballot_does_not_count_in_an_average_block() {
        use crate::scoring::{aggregate_with, BlockScoring, ScoringMode, NORM_HI, NORM_LO};
        let eligible = vec![ranking(1), ranking(2)];
        let config = BlockScoring {
            mode: ScoringMode::Average,
            ..BlockScoring::default()
        };
        let with = |items| {
            aggregate_with(
                &ballots_with_items(&eligible, items),
                NORM_LO,
                NORM_HI,
                None,
                config,
            )
        };
        // Both rankers placed entity 100 first: two ballots.
        let both = with(vec![item(1, 100), item(2, 100)]);
        assert_eq!(both[0].ballots, 2);
        // Ranker 2 cleared theirs: one ballot, the same score as if ranker 2
        // had never submitted.
        let cleared = with(vec![item(1, 100)]);
        let never = aggregate_with(
            &ballots_with_items(&eligible[..1], vec![item(1, 100)]),
            NORM_LO,
            NORM_HI,
            None,
            config,
        );
        assert_eq!(cleared[0].ballots, 1);
        assert_eq!(cleared, never);
    }
}
