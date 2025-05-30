use std::sync::Arc;

use futures::future::join_all;
use stream::utils::BlockMetadata;

use crate::cache::properties_cache::ImmutableCache;
use crate::models::properties::PropertiesModel;
use crate::models::relations::RelationsModel;
use crate::models::{entities::EntitiesModel, values::ValuesModel};
use crate::storage::StorageBackend;
use crate::{cache::PreprocessedEdit, error::IndexingError};

pub async fn run<S, C>(
    output: Vec<PreprocessedEdit>,
    block_metadata: &BlockMetadata,
    storage: &Arc<S>,
    properties_cache: &Arc<C>,
) -> Result<(), IndexingError>
where
    S: StorageBackend + Send + Sync + 'static,
    C: ImmutableCache + Send + Sync + 'static,
{
    println!(
        "Block #{} – Drift {}s – Edits Published {}",
        block_metadata.block_number,
        block_metadata.timestamp,
        output.len()
    );

    for preprocessed_edit in output {
        let storage = storage.clone();
        let block = block_metadata.clone();

        let handle = tokio::spawn({
            let preprocessed_edit = preprocessed_edit.clone();
            let storage = storage.clone();
            let cache = properties_cache.clone();
            let block = block.clone();

            let mut handles = Vec::new();

            async move {
                // The Edit might be malformed. The Cache still stores it with an
                // is_errored flag to denote that the entry exists but can't be
                // decoded.
                if !preprocessed_edit.is_errored {
                    let edit = preprocessed_edit.edit.unwrap();
                    let space_id = preprocessed_edit.space_id;

                    // We write properties first to update the cache with any properties
                    // created within the edit. This makes it simpler to do validation
                    // later in the edit handler as the properties cache will already
                    // be up-to-date.
                    let properties = PropertiesModel::map_edit_to_properties(&edit);

                    // For now we write properties to an in-memory cache that we reference
                    // when validating values in the edit. There's a weird mismatch between
                    // where properties data lives. We store properties on disk in order
                    // to be able to query properties. We need to do this in "real-time" as
                    // our external API depends on being able to query for properties when
                    // querying for values.
                    //
                    // This does mean we write properties in two places, one for the cache,
                    // and one for the queryable store. Eventually I think we want to move
                    // to in-memory for _all_ data stores with a disk-based commit log, but
                    // for now we'll write properties twice.
                    for property in &properties {
                        cache.insert(&property.id, property.data_type.clone()).await;
                    }

                    if let Err(error) = storage.insert_properties(&properties).await {
                        println!("Error writing properties: {}", error);
                    }

                    {
                        let edit = edit.clone();
                        let block = block.clone();
                        let storage = storage.clone();

                        handles.push(tokio::spawn(async move {
                            let entities = EntitiesModel::map_edit_to_entities(&edit, &block);

                            if let Err(error) = storage.insert_entities(&entities).await {
                                eprintln!("Error writing entities: {}", error);
                            }
                        }));
                    }

                    let (created_values, deleted_values) =
                        ValuesModel::map_edit_to_values(&edit, &space_id);

                    {
                        let storage = storage.clone();

                        handles.push(tokio::spawn(async move {
                            let write_values_result = storage.insert_values(&created_values).await;

                            if let Err(error) = write_values_result {
                                println!("Error writing set values {}", error);
                            }
                        }));
                    }

                    {
                        let storage = storage.clone();
                        let space_id = space_id.clone();

                        handles.push(tokio::spawn(async move {
                            let write_values_result =
                                storage.delete_values(&deleted_values, &space_id).await;

                            if let Err(error) = write_values_result {
                                println!("Error writing set values {}", error);
                            }
                        }));
                    }

                    let (
                        created_relations,
                        updated_relations,
                        unset_relations,
                        deleted_relation_ids,
                    ) = RelationsModel::map_edit_to_relations(&edit, &space_id);

                    {
                        let storage = storage.clone();

                        handles.push(tokio::spawn(async move {
                            let write_relations_result =
                                storage.insert_relations(&created_relations).await;

                            if let Err(write_error) = write_relations_result {
                                println!("Error writing relations {}", write_error);
                            }
                        }));
                    }

                    {
                        let storage = storage.clone();

                        handles.push(tokio::spawn(async move {
                            let update_relations_result =
                                storage.update_relations(&updated_relations).await;

                            if let Err(write_error) = update_relations_result {
                                println!("Error updating relations {}", write_error);
                            }
                        }));
                    }

                    {
                        let storage = storage.clone();

                        handles.push(tokio::spawn(async move {
                            let unset_relations_result =
                                storage.unset_relation_fields(&unset_relations).await;

                            if let Err(write_error) = unset_relations_result {
                                println!("Error unsetting relation fields {}", write_error);
                            }
                        }));
                    }

                    {
                        let storage = storage.clone();

                        handles.push(tokio::spawn(async move {
                            let delete_relations_result = storage
                                .delete_relations(&deleted_relation_ids, &space_id)
                                .await;

                            if let Err(write_error) = delete_relations_result {
                                println!("Error deleting relations {}", write_error);
                            }
                        }));
                    }
                }

                join_all(handles).await;
            }
        })
        .await;

        match handle {
            Ok(_) => {
                //
            }
            Err(error) => println!(
                "[Root handler] Error executing task {} for edit {:?}",
                error, preprocessed_edit
            ),
        }
    }

    Ok(())
}
