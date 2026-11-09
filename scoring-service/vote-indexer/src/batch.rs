//! One vote batch against Postgres: read the stored tallies under row locks, write the raw votes,
//! user votes, tallies and score values in one transaction, then refresh the ranking scores.
//!
//! `VOTE_INDEXER_PG_WRITES` (default on) switches all of it off; see [`process_vote_batch`].

use std::collections::HashMap;

use hermes_instrumentation::{debug, info_span, warn, Instrument};

use crate::error::IndexerError;
use crate::handlers::voting::{build_score_values, calculate_vote_counts, get_latest_user_votes};
use crate::metrics;
use crate::models::voting::{UserVoteCriteria, VoteCountCriteria, VoteItem};
use crate::storage::Storage;

/// Process a batch of votes within a single transaction.
///
/// With `VOTE_INDEXER_PG_WRITES` off the whole batch is skipped before any statement (no
/// transaction, no locking read, no write, no ranking refresh) and counted once under
/// `vote_batch`; it reports every vote as written so the offsets commit. The Postgres tallies
/// are what the geo-db mirror and the reads shadow are computed from, so they stop with it.
pub async fn process_vote_batch(
    votes: &[VoteItem],
    storage: &Storage,
) -> Result<usize, IndexerError> {
    if votes.is_empty() {
        return Ok(0);
    }
    if !storage.pg_writes_enabled() {
        metrics::pg_write_skipped(metrics::PgWriteSite::VoteBatch);
        return Ok(votes.len());
    }

    let span = info_span!(
        "vote_indexer.process_batch",
        vote_count = votes.len(),
        user_votes = tracing::field::Empty,
        vote_counts = tracing::field::Empty,
    );

    async {
        let vote_count = votes.len();

        // Get deduplicated user votes from this batch
        let user_votes = get_latest_user_votes(votes);

        // Build criteria for fetching existing data
        let user_vote_criteria: Vec<UserVoteCriteria> = user_votes
            .iter()
            .map(|v| (v.voter_id, v.object_id, v.space_id, v.object_type, v.kind))
            .collect();

        let vote_count_criteria: Vec<VoteCountCriteria> = user_votes
            .iter()
            .map(|v| (v.object_id, v.space_id, v.object_type, v.kind))
            .collect();

        // Start transaction before reads to ensure consistency.
        // Reads use FOR UPDATE to lock rows and prevent concurrent modifications.
        let mut tx = storage.pool().begin().await?;

        // Fetch existing user votes and vote counts (with row locks)
        let stored_user_votes = storage
            .get_user_votes_tx(&user_vote_criteria, &mut tx)
            .await?;
        let stored_vote_counts = storage
            .get_votes_counts_tx(&vote_count_criteria, &mut tx)
            .await?;

        // Convert to HashMaps for lookup
        let stored_user_votes_map: HashMap<UserVoteCriteria, _> = stored_user_votes
            .into_iter()
            .map(|v| {
                (
                    (v.voter_id, v.object_id, v.space_id, v.object_type, v.kind),
                    v,
                )
            })
            .collect();

        let shadow_before = storage
            .has_reads_shadow()
            .then(|| stored_vote_counts.clone());
        let stored_vote_counts_map: HashMap<VoteCountCriteria, _> = stored_vote_counts
            .into_iter()
            .map(|v| ((v.object_id, v.space_id, v.object_type, v.kind), v))
            .collect();

        // Calculate updated vote counts
        let updated_vote_counts =
            calculate_vote_counts(&user_votes, &stored_user_votes_map, &stored_vote_counts_map);

        // Record computed values in current span
        use tracing::Span;
        Span::current().record("user_votes", user_votes.len());
        Span::current().record("vote_counts", updated_vote_counts.len());

        // Insert raw votes (audit log)
        storage.insert_votes(votes, &mut tx).await?;

        // Upsert user votes (current state)
        storage.upsert_user_votes(&user_votes, &mut tx).await?;

        // Upsert vote counts (aggregates)
        storage
            .upsert_votes_counts(&updated_vote_counts, &mut tx)
            .await?;

        // Mirror entity net scores into `values` under the Score system property
        // so `entities_ordered_by_property` can sort by raw score with no SQL changes.
        let score_values = build_score_values(&updated_vote_counts);
        storage.upsert_score_values(&score_values, &mut tx).await?;

        tx.commit().await?;

        // Compare the batch's reads with geo-db (flag off by default); background, counts only.
        if let Some(before) = shadow_before {
            storage.shadow_votes_counts(
                vote_count_criteria.clone(),
                before,
                updated_vote_counts.clone(),
            );
        }

        // Copy the committed tallies into geo-db (flag off by default); best-effort.
        storage.mirror_votes_counts(&updated_vote_counts).await;

        // Recompute the Explore feed ranking score for entities whose curation votes
        // changed. Deliberately AFTER the commit and non-fatal: a stale score is
        // harmless and self-correcting, a lost vote is not. This also makes deploy
        // order irrelevant if the migration has not landed yet.
        if let Err(e) = storage.refresh_ranking_scores(&updated_vote_counts).await {
            metrics::ranking_refresh_failed();
            warn!(
                error = %e,
                "Failed to refresh feed ranking scores; votes are committed and the \
                 scores will be corrected by the next vote or a backfill run"
            );
        }

        debug!(
            raw_votes = vote_count,
            user_votes = user_votes.len(),
            vote_counts = updated_vote_counts.len(),
            "Processed vote batch"
        );

        Ok(vote_count)
    }
    .instrument(span)
    .await
}
