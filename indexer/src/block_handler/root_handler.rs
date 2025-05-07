use std::sync::Arc;

use futures::future::join_all;
use grc20::pb::chain::GeoOutput;
use stream::utils::BlockMetadata;
use tokio::task;
use tokio_retry::{
    strategy::{jitter, ExponentialBackoff},
    Retry,
};

use crate::error::IndexingError;
use crate::storage::StorageBackend;
use crate::{cache::CacheBackend, storage::entities::EntitiesModel};

pub async fn run<S, C>(
    output: &GeoOutput,
    block_metadata: &BlockMetadata,
    storage: &Arc<S>,
    cache: &Arc<C>,
) -> Result<(), IndexingError>
where
    S: StorageBackend + 'static,
    C: CacheBackend + 'static,
{
    println!(
        "Block #{} – Drift {}s – Edits Published {}",
        block_metadata.block_number,
        block_metadata.timestamp,
        output.edits_published.len()
    );

    let mut handles = Vec::new();

    for edit in output.edits_published.clone() {
        let storage = storage.clone();
        let cache = cache.clone();
        let block = block_metadata.clone();

        let handle = task::spawn(async move {
            // We retry requests to the cache in the case that the cache is
            // still populating. For now we assume writing to + reading from
            // the cache can't fail
            let retry = ExponentialBackoff::from_millis(10)
                .factor(2)
                .max_delay(std::time::Duration::from_secs(5))
                .map(jitter);
            let edit = Retry::spawn(retry, async || cache.get(&edit.content_uri).await).await;

            match edit {
                Ok(value) => {
                    if !value.is_errored {
                        let entities =
                            EntitiesModel::map_edit_to_entities(&value.edit.unwrap(), &block);

                        let result = storage.insert_entities(&entities).await;

                        match result {
                            Ok(value) => {}
                            Err(error) => {
                                println!("Error writing {}", error);
                            }
                        }
                    }
                }
                Err(error) => {
                    //
                }
            }

            Ok::<(), IndexingError>(())
        });

        handles.push(handle);
    }

    // Wait for all processing in the current block to finish before continuing
    // to the next block
    let done = join_all(handles).await;

    Ok(())
}
