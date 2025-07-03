use std::sync::Arc;

use stream::utils::BlockMetadata;

use crate::block_handler::utils::handle_task_result;
use crate::{
    error::IndexingError, models::membership::MembershipModel, storage::StorageBackend,
    AddedMember, RemovedMember,
};

pub async fn run<S>(
    added_members: &Vec<AddedMember>,
    removed_members: &Vec<RemovedMember>,
    added_editors: &Vec<AddedMember>,
    removed_editors: &Vec<RemovedMember>,
    _block_metadata: &BlockMetadata,
    storage: &Arc<S>,
) -> Result<(), IndexingError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let members_task = {
        let storage = Arc::clone(storage);
        let added_members = added_members.clone();
        let removed_members = removed_members.clone();
        tokio::spawn(async move {
            let mut tx = storage.get_pool().begin().await?;

            // Process added members
            if !added_members.is_empty() {
                let members_to_add = MembershipModel::map_added_members(&added_members);
                storage.insert_members(&members_to_add, &mut tx).await?;
            }

            // Process removed members
            if !removed_members.is_empty() {
                let members_to_remove = MembershipModel::map_removed_members(&removed_members);
                storage.remove_members(&members_to_remove, &mut tx).await?;
            }

            tx.commit().await?;
            Ok(())
        })
    };

    let editors_task = {
        let storage = Arc::clone(storage);
        let added_editors = added_editors.clone();
        let removed_editors = removed_editors.clone();
        tokio::spawn(async move {
            let mut tx = storage.get_pool().begin().await?;

            // Process added editors
            if !added_editors.is_empty() {
                let editors_to_add = MembershipModel::map_added_editors(&added_editors);
                storage.insert_editors(&editors_to_add, &mut tx).await?;
            }

            // Process removed editors
            if !removed_editors.is_empty() {
                let editors_to_remove = MembershipModel::map_removed_editors(&removed_editors);
                storage.remove_editors(&editors_to_remove, &mut tx).await?;
            }

            tx.commit().await?;
            Ok(())
        })
    };

    let (members_result, editors_result) = tokio::join!(members_task, editors_task);

    handle_task_result(members_result)?;
    handle_task_result(editors_result)?;

    Ok(())
}
