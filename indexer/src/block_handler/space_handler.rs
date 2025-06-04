use std::sync::Arc;

use stream::utils::BlockMetadata;

use crate::{
    error::IndexingError, models::spaces::SpacesModel, storage::StorageBackend, CreatedSpace,
};

pub async fn run<S>(
    output: Vec<CreatedSpace>,
    block_metadata: &BlockMetadata,
    storage: &Arc<S>,
) -> Result<(), IndexingError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let created_spaces = SpacesModel::map_created_spaces(&output);
    storage.clone().insert_spaces(&created_spaces).await;

    Ok(())
}
