use std::sync::Arc;

use stream::utils::BlockMetadata;

use crate::block_handler::{edit_handler, space_handler};
use crate::cache::properties_cache::ImmutableCache;

use crate::error::IndexingError;
use crate::storage::StorageBackend;
use crate::KgData;

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

    let edit_result =
        edit_handler::run(&output.edits, block_metadata, storage, properties_cache).await;
    let space_result = space_handler::run(&output.spaces, block_metadata, storage).await;

    Ok(())
}
