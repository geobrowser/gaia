use std::sync::Arc;

use futures::future::join_all;
use grc20::pb::chain::GeoOutput;
use stream::utils::BlockMetadata;
use tokio::task;
use tokio_retry::{
    strategy::{jitter, ExponentialBackoff},
    Retry,
};

use crate::storage::StorageBackend;
use crate::{cache::CacheBackend, storage::entities::EntitiesModel};
use crate::{error::IndexingError, storage::triples::TriplesModel};

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

    for chain_edit in output.edits_published.clone() {
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
            let cached_edit_entry =
                Retry::spawn(retry, async || cache.get(&chain_edit.content_uri).await).await?;

            // The Edit might be malformed. The Cache still stores it with an
            // is_errored flag to denote that the entry exists but can't be
            // decoded.
            if !cached_edit_entry.is_errored {
                let edit = cached_edit_entry.edit.unwrap();
                let space_id = cached_edit_entry.space_id;

                // @TODO: transaction with non-blocking writes
                let entities = EntitiesModel::map_edit_to_entities(&edit, &block);
                let result = storage.insert_entities(&entities).await;

                if let Err(error) = result {
                    println!("Error writing entities {}", error);
                }

                let triples = TriplesModel::map_edit_to_triples(&edit, &space_id);
                let write_result = storage.insert_triples(&triples.0).await;

                if let Err(write_error) = write_result {
                    println!("Error writing triples {}", write_error);
                }

                let delete_result = storage.delete_triples(&triples.1).await;

                if let Err(delete_error) = delete_result {
                    println!("Error deleting triples {}", delete_error);
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
