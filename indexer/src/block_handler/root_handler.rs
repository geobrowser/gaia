use std::sync::Arc;

use futures::future::join_all;
use stream::utils::BlockMetadata;
use tokio::task;

use crate::models::relations::RelationsModel;
use crate::models::{entities::EntitiesModel, properties::ValuesModel};
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
                // @TODO: spawn tasks and join for each mapping
                let entities = EntitiesModel::map_edit_to_entities(&edit, &block);
                let result = storage.insert_entities(&entities).await;

                if let Err(error) = result {
                    println!("Error writing entities {}", error);
                }

                let (created_values, deleted_values) =
                    ValuesModel::map_edit_to_values(&edit, &space_id);
                let write_properties_result = storage.insert_values(&created_values).await;

                if let Err(write_error) = write_properties_result {
                    println!("Error writing properties {}", write_error);
                }

                let delete_properties_result = storage.delete_values(&deleted_values).await;

                if let Err(delete_error) = delete_properties_result {
                    println!("Error deleting properties {}", delete_error);
                }

                let (created_relations, updated_relations, deleted_relation_ids) =
                    RelationsModel::map_edit_to_relations(&edit, &space_id);

                let write_relations_result = storage.insert_relations(&created_relations).await;

                if let Err(write_error) = write_relations_result {
                    println!("Error writing relations {}", write_error);
                }

                let delete_relations_result = storage.delete_relations(&deleted_relation_ids).await;

                if let Err(write_error) = delete_relations_result {
                    println!("Error deleting relations {}", write_error);
                }

                // @TODO: Delete entities
                // We can filter out any writes for entities that are also deleted in the same edit
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
