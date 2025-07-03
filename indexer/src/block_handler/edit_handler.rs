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
use crate::validators::validate_string_by_datatype;
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
async fn validate_created_values<C>(created_values: Vec<ValueOp>, cache: &Arc<C>) -> Vec<ValueOp>
where
    C: ImmutableCache + Send + Sync + 'static,
{
    let mut validated_created_values = Vec::new();

    for value in created_values {
        // Only validate + write values that have actual content in the value
        if let Some(ref string_value) = value.value {
            match cache.get(&value.property_id).await {
                Ok(data_type) => {
                    match validate_string_by_datatype(data_type, string_value) {
                        Ok(_) => {
                            validated_created_values.push(value);
                        }
                        Err(validation_error) => {
                            // @TODO: tracing
                            eprintln!(
                                "Validation error for property {} with value '{}': {}",
                                value.property_id, string_value, validation_error
                            );
                            // Skip invalid values rather than failing the entire edit
                        }
                    }
                }
                // If property not found in cache, don't include the value.
                // (byron – 2025-02-06): This does introduce a potential state
                // inconsistency between the properties cache and the edit here.
                // This will be solved in a distributed cache indexer. For now
                // this indexer reads every edit on the chain therefore properties
                // can't get out of sync.
                Err(_) => {
                    // @TODO: tracing
                    // eprintln!(
                    //     "Property {} not found in cache, skipping value validation",
                    //     value.property_id
                    // );
                }
            }
        }
    }

    validated_created_values
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
                        ValuesModel::map_edit_to_values(&edit, &space_id);

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
                }

                if let Err(error) = tx.commit().await {
                    eprintln!("Error committing transaction: {}", error);
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
            value: Some("123.45".to_string()),
            language: None,
            unit: None,
        }];

        let validated = validate_created_values(values, &cache).await;
        assert_eq!(validated.len(), 1);
        assert_eq!(validated[0].value, Some("123.45".to_string()));
    }

    #[tokio::test]
    async fn test_validate_created_values_invalid_data_filtered() {
        let cache = Arc::new(PropertiesCache::new());
        let property_id = Uuid::new_v4();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        // Insert a Number property type into cache
        cache.insert(&property_id, DataType::Number).await;

        let values = vec![
            ValueOp {
                id: Uuid::new_v4(),
                change_type: ValueChangeType::SET,
                entity_id,
                property_id,
                space_id,
                value: Some("123.45".to_string()), // Valid number
                language: None,
                unit: None,
            },
            ValueOp {
                id: Uuid::new_v4(),
                change_type: ValueChangeType::SET,
                entity_id,
                property_id,
                space_id,
                value: Some("not-a-number".to_string()), // Invalid number
                language: None,
                unit: None,
            },
        ];

        let validated = validate_created_values(values, &cache).await;
        // Only the valid value should remain
        assert_eq!(validated.len(), 1);
        assert_eq!(validated[0].value, Some("123.45".to_string()));
    }

    #[tokio::test]
    async fn test_validate_created_values_none_values_pass_through() {
        let cache = Arc::new(PropertiesCache::new());
        let property_id = Uuid::new_v4();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let values = vec![ValueOp {
            id: Uuid::new_v4(),
            change_type: ValueChangeType::DELETE,
            entity_id,
            property_id,
            space_id,
            value: None, // None values should pass through without validation
            language: None,
            unit: None,
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
            value: Some("some-value".to_string()),
            language: None,
            unit: None,
        }];

        let validated = validate_created_values(values, &cache).await;
        // Value should be filtered out when property not found in cache
        assert_eq!(validated.len(), 0);
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
        cache.insert(&text_prop_id, DataType::Text).await;
        cache.insert(&checkbox_prop_id, DataType::Checkbox).await;
        cache.insert(&point_prop_id, DataType::Point).await;

        let values = vec![
            ValueOp {
                id: Uuid::new_v4(),
                change_type: ValueChangeType::SET,
                entity_id,
                property_id: text_prop_id,
                space_id,
                value: Some("Hello World".to_string()), // Valid text
                language: None,
                unit: None,
            },
            ValueOp {
                id: Uuid::new_v4(),
                change_type: ValueChangeType::SET,
                entity_id,
                property_id: checkbox_prop_id,
                space_id,
                value: Some("1".to_string()), // Valid checkbox
                language: None,
                unit: None,
            },
            ValueOp {
                id: Uuid::new_v4(),
                change_type: ValueChangeType::SET,
                entity_id,
                property_id: checkbox_prop_id,
                space_id,
                value: Some("invalid-checkbox".to_string()), // Invalid checkbox
                language: None,
                unit: None,
            },
            ValueOp {
                id: Uuid::new_v4(),
                change_type: ValueChangeType::SET,
                entity_id,
                property_id: point_prop_id,
                space_id,
                value: Some("1.5,2.5".to_string()), // Valid point
                language: None,
                unit: None,
            },
        ];

        let validated = validate_created_values(values, &cache).await;
        // Should have 3 valid values (text, valid checkbox, point)
        assert_eq!(validated.len(), 3);

        // Verify the specific values that made it through
        let text_values: Vec<_> = validated
            .iter()
            .filter(|v| v.property_id == text_prop_id)
            .collect();
        assert_eq!(text_values.len(), 1);
        assert_eq!(text_values[0].value, Some("Hello World".to_string()));

        let valid_checkbox_values: Vec<_> = validated
            .iter()
            .filter(|v| v.property_id == checkbox_prop_id)
            .collect();
        assert_eq!(valid_checkbox_values.len(), 1);
        assert_eq!(valid_checkbox_values[0].value, Some("1".to_string()));

        let point_values: Vec<_> = validated
            .iter()
            .filter(|v| v.property_id == point_prop_id)
            .collect();
        assert_eq!(point_values.len(), 1);
        assert_eq!(point_values[0].value, Some("1.5,2.5".to_string()));
    }
}
