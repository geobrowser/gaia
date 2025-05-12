use std::sync::Arc;

use futures::future::join_all;
use stream::utils::BlockMetadata;
use tokio::task;

use crate::models::{entities::EntitiesModel, properties::PropertiesModel};
use crate::storage::StorageBackend;
use crate::{cache::PreprocessedEdit, error::IndexingError};

pub async fn run<S>(
    output: Vec<PreprocessedEdit>,
    block_metadata: &BlockMetadata,
    storage: &Arc<S>,
) -> Result<(), IndexingError>
where
    S: StorageBackend + 'static,
{
    println!(
        "Block #{} – Drift {}s – Edits Published {}",
        block_metadata.block_number,
        block_metadata.timestamp,
        output.len()
    );

    let mut handles = Vec::new();

    for preprocessed_edit in output {
        let storage = storage.clone();
        let block = block_metadata.clone();

        let handle = task::spawn(async move {
            // The Edit might be malformed. The Cache still stores it with an
            // is_errored flag to denote that the entry exists but can't be
            // decoded.
            if !preprocessed_edit.is_errored {
                let edit = preprocessed_edit.edit.unwrap();
                let space_id = preprocessed_edit.space_id;

                // @TODO: transaction with non-blocking writes
                let entities = EntitiesModel::map_edit_to_entities(&edit, &block);
                let result = storage.insert_entities(&entities).await;

                if let Err(error) = result {
                    println!("Error writing entities {}", error);
                }

                let properties = PropertiesModel::map_edit_to_properties(&edit, &space_id);
                let write_result = storage.insert_properties(&properties.0).await;

                if let Err(write_error) = write_result {
                    println!("Error writing triples {}", write_error);
                }

                let delete_result = storage.delete_properties(&properties.1).await;

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
