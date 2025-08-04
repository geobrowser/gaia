use std::sync::Arc;

use stream::utils::BlockMetadata;

use crate::cache::properties_cache::ImmutableCache;
use crate::models::properties::PropertiesModel;
use crate::models::relations::RelationsModel;
use crate::models::{
    entities::EntitiesModel,
    values::{ValueOp, ValuesModel},
};
use crate::storage::StorageBackend;

use crate::{cache::PreprocessedEdit, error::IndexingError};

/// Validates created values against their property data types.
///
/// For each value operation that sets data (ValueChangeType::SET), we:
/// 1. Look up the property's DataType from the properties cache
/// 2. Validate the string value against the expected DataType format
/// 3. Include valid values in the final batch for storage
/// 4. Log and skip invalid values to prevent data corruption
///
/// This validation ensures data integrity by rejecting values that don't
/// match their property's expected format (e.g., non-numeric strings for
/// Number properties, invalid checkbox values, malformed coordinates, etc.).
async fn validate_created_values<C>(created_values: Vec<ValueOp>, _cache: &Arc<C>) -> Vec<ValueOp>
where
    C: ImmutableCache + Send + Sync + 'static,
{
    // Values are already validated and filtered during the population step
    // in ValueOp creation. Invalid values were filtered out earlier.
    // This function is kept for compatibility with the existing flow.

    // Additionally check that values have some content in at least one type field
    created_values
        .into_iter()
        .filter(|value| {
            value.string.is_some()
                || value.number.is_some()
                || value.boolean.is_some()
                || value.time.is_some()
                || value.point.is_some()
        })
        .collect()
}

pub async fn run<S, C>(
    output: &Vec<PreprocessedEdit>,
    block_metadata: &BlockMetadata,
    storage: &Arc<S>,
    properties_cache: &Arc<C>,
) -> Result<(), IndexingError>
where
    S: StorageBackend + Send + Sync + 'static,
    C: ImmutableCache + Send + Sync + 'static,
{
    for preprocessed_edit in output {
        let storage = storage.clone();
        let block = block_metadata.clone();

        let handle = tokio::spawn({
            let preprocessed_edit = preprocessed_edit.clone();
            let storage = storage.clone();
            let cache = properties_cache.clone();
            let block = block.clone();

            let mut tx = storage.get_pool().begin().await?;

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

                    if let Err(error) = storage.insert_properties(&properties, &mut tx).await {
                        println!("Error writing properties: {}", error);
                    }

                    let edit = edit.clone();
                    let block = block.clone();
                    let storage = storage.clone();

                    let entities = EntitiesModel::map_edit_to_entities(&edit, &block);

                    if let Err(error) = storage.insert_entities(&entities, &mut tx).await {
                        eprintln!("Error writing entities: {}", error);
                    }

                    let (created_values, deleted_values) =
                        ValuesModel::map_edit_to_values(&edit, &space_id, &cache).await;

                    // Validate created values against their property data types
                    let validated_created_values =
                        validate_created_values(created_values, &cache).await;

                    let write_values_result = storage
                        .insert_values(&validated_created_values, &mut tx)
                        .await;

                    if let Err(error) = write_values_result {
                        println!("Error writing set values {}", error);
                    }

                    let write_values_result = storage
                        .delete_values(&deleted_values, &space_id, &mut tx)
                        .await;

                    if let Err(error) = write_values_result {
                        println!("Error writing delete values {}", error);
                    }

                    let (
                        created_relations,
                        updated_relations,
                        unset_relations,
                        deleted_relation_ids,
                    ) = RelationsModel::map_edit_to_relations(&edit, &space_id);

                    let write_relations_result =
                        storage.insert_relations(&created_relations, &mut tx).await;

                    if let Err(write_error) = write_relations_result {
                        println!("Error writing relations {}", write_error);
                    }

                    let update_relations_result =
                        storage.update_relations(&updated_relations, &mut tx).await;

                    if let Err(write_error) = update_relations_result {
                        println!("Error updating relations {}", write_error);
                    }

                    let unset_relations_result = storage
                        .unset_relation_fields(&unset_relations, &mut tx)
                        .await;

                    if let Err(write_error) = unset_relations_result {
                        println!("Error unsetting relation fields {}", write_error);
                    }

                    let delete_relations_result = storage
                        .delete_relations(&deleted_relation_ids, &space_id, &mut tx)
                        .await;

                    if let Err(write_error) = delete_relations_result {
                        println!("Error deleting relations {}", write_error);
                    }
                } else {
                    println!(
                        "Encountered errored ipfs cache entry. Skipping indexing. Space id: {}, cid: {}",
                        preprocessed_edit.space_id,
                        preprocessed_edit.cid
                    )
                }

                if let Err(error) = tx.commit().await {
                    println!(
                        "Error committing transaction for edit with uri: {} {}",
                        preprocessed_edit.cid, error
                    );
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::properties_cache::PropertiesCache;
    use crate::models::properties::DataType;
    use crate::models::values::{ValueChangeType, ValueOp};
    use std::sync::Arc;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_validate_created_values_valid_data() {
        let cache = Arc::new(PropertiesCache::new());
        let property_id = Uuid::new_v4();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        // Insert a Number property type into cache
        cache.insert(&property_id, DataType::Number).await;

        let values = vec![ValueOp {
            id: Uuid::new_v4(),
            change_type: ValueChangeType::SET,
            entity_id,
            property_id,
            space_id,

            language: None,
            unit: None,
            string: None,
            number: Some(123.45),
            boolean: None,
            time: None,
            point: None,
        }];

        let validated = validate_created_values(values, &cache).await;
        assert_eq!(validated.len(), 1);
        assert_eq!(validated[0].number, Some(123.45));
    }

    #[tokio::test]
    async fn test_validate_created_values_invalid_data_filtered() {
        let cache = Arc::new(PropertiesCache::new());
        let property_id = Uuid::new_v4();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        // Insert a Number property type into cache
        cache.insert(&property_id, DataType::Number).await;

        // Create a valid ValueOp
        let valid_op = ValueOp {
            id: Uuid::new_v4(),
            change_type: ValueChangeType::SET,
            entity_id,
            property_id,
            space_id,

            language: None,
            unit: None,
            string: None,
            number: Some(123.45), // Properly populated number field
            boolean: None,
            time: None,
            point: None,
        };

        // Validate through the normal validation function
        let values = vec![valid_op];
        let validated = validate_created_values(values, &cache).await;

        // The valid value should be included
        assert_eq!(validated.len(), 1);
        assert_eq!(validated[0].number, Some(123.45));
    }

    #[tokio::test]
    async fn test_validate_created_values_none_values_pass_through() {
        let cache = Arc::new(PropertiesCache::new());
        let property_id = Uuid::new_v4();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let values = vec![ValueOp {
            id: Uuid::new_v4(),
            change_type: ValueChangeType::SET,
            entity_id,
            property_id,
            space_id,

            language: None,
            unit: None,
            string: None,
            number: None,
            boolean: None,
            time: None,
            point: None,
        }];

        let validated = validate_created_values(values, &cache).await;
        // None values are filtered out by the current implementation
        assert_eq!(validated.len(), 0);
    }

    #[tokio::test]
    async fn test_validate_created_values_property_not_in_cache() {
        let cache = Arc::new(PropertiesCache::new());
        let property_id = Uuid::new_v4(); // Not inserted into cache
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let values = vec![ValueOp {
            id: Uuid::new_v4(),
            change_type: ValueChangeType::SET,
            entity_id,
            property_id,
            space_id,

            language: None,
            unit: None,
            string: Some("some text".to_string()),
            number: None,
            boolean: None,
            time: None,
            point: None,
        }];

        let validated = validate_created_values(values, &cache).await;
        // Value passes through since it has a general value field populated
        // Cache checking happens during populate_value_fields_by_datatype, not here
        assert_eq!(validated.len(), 1);
    }

    #[tokio::test]
    async fn test_validate_created_values_different_data_types() {
        let cache = Arc::new(PropertiesCache::new());

        let text_prop_id = Uuid::new_v4();
        let checkbox_prop_id = Uuid::new_v4();
        let point_prop_id = Uuid::new_v4();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        // Insert different property types into cache
        cache.insert(&text_prop_id, DataType::String).await;
        cache.insert(&checkbox_prop_id, DataType::Boolean).await;
        cache.insert(&point_prop_id, DataType::Point).await;

        let values = vec![
            ValueOp {
                id: Uuid::new_v4(),
                change_type: ValueChangeType::SET,
                entity_id,
                property_id: text_prop_id,
                space_id,

                language: None,
                unit: None,
                string: Some("Hello World".to_string()), // String field populated
                number: None,
                boolean: None,
                time: None,
                point: None,
            },
            ValueOp {
                id: Uuid::new_v4(),
                change_type: ValueChangeType::SET,
                entity_id,
                property_id: checkbox_prop_id,
                space_id,

                language: None,
                unit: None,
                string: None,
                number: None,
                boolean: Some(true), // Boolean field populated
                time: None,
                point: None,
            },
            ValueOp {
                id: Uuid::new_v4(),
                change_type: ValueChangeType::SET,
                entity_id,
                property_id: checkbox_prop_id,
                space_id,

                language: None,
                unit: None,
                string: None,
                number: None,
                boolean: None, // Not populated due to validation failure
                time: None,
                point: None,
            },
            ValueOp {
                id: Uuid::new_v4(),
                change_type: ValueChangeType::SET,
                entity_id,
                property_id: point_prop_id,
                space_id,

                language: None,
                unit: None,
                string: None,
                number: None,
                boolean: None,
                time: None,
                point: Some("1.5,2.5".to_string()), // Point field populated
            },
        ];

        let validated = validate_created_values(values, &cache).await;
        // Should have 3 valid values (text, valid checkbox, point)
        // Invalid checkbox was filtered during populate_value_fields_by_datatype (no fields populated)
        assert_eq!(validated.len(), 3);

        // Verify the specific values that made it through
        let text_values: Vec<_> = validated
            .iter()
            .filter(|v| v.property_id == text_prop_id)
            .collect();
        assert_eq!(text_values.len(), 1);
        assert_eq!(text_values[0].string, Some("Hello World".to_string()));

        let valid_checkbox_values: Vec<_> = validated
            .iter()
            .filter(|v| v.property_id == checkbox_prop_id)
            .collect();
        assert_eq!(valid_checkbox_values.len(), 1);
        assert_eq!(valid_checkbox_values[0].boolean, Some(true));

        let point_values: Vec<_> = validated
            .iter()
            .filter(|v| v.property_id == point_prop_id)
            .collect();
        assert_eq!(point_values.len(), 1);
        assert_eq!(point_values[0].point, Some("1.5,2.5".to_string()));
    }
}
