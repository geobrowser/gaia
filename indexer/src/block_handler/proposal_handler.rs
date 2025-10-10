use std::sync::Arc;

use stream::utils::BlockMetadata;

use crate::{
    error::IndexingError, models::proposals::ProposalsModel, storage::StorageBackend,
    ExecutedProposal, ProposalCreated,
};

pub async fn run<S>(
    created_proposals: &Vec<ProposalCreated>,
    executed_proposals: &Vec<ExecutedProposal>,
    block_metadata: &BlockMetadata,
    storage: &Arc<S>,
) -> Result<(), IndexingError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let mut tx = storage.get_pool().begin().await?;

    // Insert new proposals
    if !created_proposals.is_empty() {
        let block_number = block_metadata.block_number as i64;
        let proposal_items = ProposalsModel::map_created_proposals(created_proposals, block_number);
        storage.insert_proposals(&proposal_items, &mut tx).await?;
    }

    // Update executed proposal statuses
    if !executed_proposals.is_empty() {
        let executed_proposal_ids = ProposalsModel::map_executed_proposals(executed_proposals);
        storage
            .update_proposal_status(&executed_proposal_ids, "executed", &mut tx)
            .await?;
    }

    tx.commit().await?;
    Ok(())
}
