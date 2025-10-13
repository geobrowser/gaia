use std::sync::Arc;

use stream::utils::BlockMetadata;
use tracing::{debug, info, instrument};

use crate::error::IndexingError;
use crate::models::votes::VotesModel;
use crate::storage::StorageBackend;
use crate::VoteCast;

#[instrument(skip_all, fields(
    vote_count = votes.len(),
    block_number = block_metadata.block_number
))]
pub async fn run<S: StorageBackend>(
    votes: &[VoteCast],
    block_metadata: &BlockMetadata,
    storage: &Arc<S>,
) -> Result<(), IndexingError> {
    if votes.is_empty() {
        debug!("No votes to process");
        return Ok(());
    }

    info!(
        vote_count = votes.len(),
        block_number = block_metadata.block_number,
        "Processing votes"
    );

    // Map VoteCast events to VoteItem database records
    let vote_items = VotesModel::map_votes(votes, block_metadata.block_number);
    
    if vote_items.is_empty() {
        debug!(
            original_count = votes.len(),
            "No valid votes to insert after mapping"
        );
        return Ok(());
    }

    debug!(
        mapped_count = vote_items.len(),
        original_count = votes.len(),
        "Mapped votes for insertion"
    );

    // Start transaction and insert votes
    let mut tx = storage.get_pool().begin().await?;
    storage.insert_votes(&vote_items, &mut tx).await?;
    tx.commit().await?;

    info!(
        inserted_count = vote_items.len(),
        block_number = block_metadata.block_number,
        "Successfully inserted votes"
    );

    Ok(())
}

// TODO: Add tests once MockStorage is implemented with vote support