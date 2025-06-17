use std::sync::Arc;

use stream::utils::BlockMetadata;

use crate::block_handler::{edit_handler, space_handler};
use crate::cache::properties_cache::ImmutableCache;

use crate::error::IndexingError;
use crate::storage::StorageBackend;
use crate::KgData;

fn handle_task_result(
    result: Result<Result<(), IndexingError>, tokio::task::JoinError>,
) -> Result<(), IndexingError> {
    match result {
        Ok(handler_result) => handler_result,
        Err(join_error) => Err(IndexingError::from(join_error)),
    }
}

pub async fn run<S, C>(
    output: &KgData,
    block_metadata: &BlockMetadata,
    storage: &Arc<S>,
    properties_cache: &Arc<C>,
) -> Result<(), IndexingError>
where
    S: StorageBackend + Send + Sync + 'static,
    C: ImmutableCache + Send + Sync + 'static,
{
    println!(
        "Block #{} – Drift {}s",
        block_metadata.block_number, block_metadata.timestamp,
    );

    let space_task = {
        let storage = Arc::clone(storage);
        let block_metadata = block_metadata.clone();
        let spaces = output.spaces.clone();
        tokio::spawn(async move {
            space_handler::run(&spaces, &block_metadata, &storage).await
        })
    };

    let edit_task = {
        let storage = Arc::clone(storage);
        let properties_cache = Arc::clone(properties_cache);
        let block_metadata = block_metadata.clone();
        let edits = output.edits.clone();
        tokio::spawn(async move {
            edit_handler::run(&edits, &block_metadata, &storage, &properties_cache).await
        })
    };

    let (space_result, edit_result) = tokio::join!(space_task, edit_task);
    
    handle_task_result(space_result)?;
    handle_task_result(edit_result)?;

    Ok(())
}
