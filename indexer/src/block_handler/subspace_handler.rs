use std::sync::Arc;

use stream::utils::BlockMetadata;

use crate::block_handler::utils::handle_task_result;
use crate::{
    error::IndexingError, models::subspaces::SubspaceModel, storage::StorageBackend,
    AddedSubspace, RemovedSubspace,
};

pub async fn run<S>(
    added_subspaces: &Vec<AddedSubspace>,
    removed_subspaces: &Vec<RemovedSubspace>,
    _block_metadata: &BlockMetadata,
    storage: &Arc<S>,
) -> Result<(), IndexingError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let subspaces_task = {
        let storage = Arc::clone(storage);
        let added_subspaces = added_subspaces.clone();
        let removed_subspaces = removed_subspaces.clone();
        tokio::spawn(async move {
            let mut tx = storage.get_pool().begin().await?;

            // Process added subspaces
            if !added_subspaces.is_empty() {
                let subspaces_to_add = SubspaceModel::map_added_subspaces(&added_subspaces);
                storage.insert_subspaces(&subspaces_to_add, &mut tx).await?;
            }

            // Process removed subspaces
            if !removed_subspaces.is_empty() {
                let subspaces_to_remove = SubspaceModel::map_removed_subspaces(&removed_subspaces);
                storage.remove_subspaces(&subspaces_to_remove, &mut tx).await?;
            }

            tx.commit().await?;
            Ok(())
        })
    };

    let subspaces_result = subspaces_task.await;
    handle_task_result(subspaces_result)?;

    Ok(())
}