use std::sync::Arc;

use futures::future::join_all;
use grc20::pb::chain::GeoOutput;
use prost::Message;
use tokio::task;
use tokio_retry::{
    strategy::{jitter, ExponentialBackoff},
    Retry,
};

use crate::storage::entities::EntitiesModel;
use crate::storage::postgres::PostgresStorage;
use crate::storage::StorageBackend;
use crate::{cache::Cache, error::IndexingError};

pub async fn run(
    // @TODO: What the minimum data we need from the block?
    block_data: &stream::pb::sf::substreams::rpc::v2::BlockScopedData,
    storage: &Arc<PostgresStorage>,
    cache: &Arc<Cache>,
) -> Result<(), IndexingError> {
    let output = stream::utils::output(block_data);
    let geo = GeoOutput::decode(output.value.as_slice())?;
    let block_metadata = stream::utils::block_metadata(block_data);

    println!(
        "Block #{} - Payload {} ({} bytes) - Drift {}s – Edits Published {}",
        block_metadata.block_number,
        output.type_url.replace("type.googleapis.com/", ""),
        output.value.len(),
        block_metadata.timestamp,
        geo.edits_published.len()
    );

    let mut handles = Vec::new();

    for edit in geo.edits_published {
        let storage = storage.clone();
        let cache = cache.clone();
        let block_metadata = stream::utils::block_metadata(block_data);

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
                        let entities = EntitiesModel::map_edit_to_entities(
                            &value.edit.unwrap(),
                            &block_metadata,
                        );

                        let result = storage.insert_entities(&entities).await;

                        // match result {
                        //     Ok(value) => {}
                        //     Err(error) => {
                        //         println!("Error writing {}", error);
                        //     }
                        // }
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
