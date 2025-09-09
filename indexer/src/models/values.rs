use indexer_utils::id;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use tracing::{debug, instrument, warn};
use uuid::Uuid;
use wire::pb::grc20::{op::Payload, options, Edit, Op};

use crate::cache::properties_cache::ImmutableCache;
use crate::validators::{validate_by_datatype, ValidatedValue};

#[derive(Clone)]
pub enum ValueChangeType {
    SET,
    DELETE,
}

#[derive(Clone)]
pub struct ValueOp {
    pub id: Uuid,
    pub change_type: ValueChangeType,
    pub entity_id: Uuid,
    pub property_id: Uuid,
    pub space_id: Uuid,
    pub language: Option<String>,
    pub unit: Option<String>,
    // Data type specific fields
    pub string: Option<String>,
    pub number: Option<f64>,
    pub boolean: Option<bool>,
    pub time: Option<String>,
    pub point: Option<String>,
}

pub struct ValuesModel;

impl ValuesModel {
    #[instrument(skip_all, fields(space_id = %space_id, op_count = edit.ops.len()))]
    pub async fn map_edit_to_values<C>(
        edit: &Edit,
        space_id: &Uuid,
        cache: &Arc<C>,
    ) -> (Vec<ValueOp>, Vec<Uuid>)
    where
        C: ImmutableCache + Send + Sync + 'static,
    {
        let mut value_ops: Vec<ValueOp> = Vec::new();

        for op in &edit.ops {
            let mut ops = value_op_from_op(op, space_id, cache).await;
            value_ops.append(&mut ops);
        }

        // A single edit may have multiple CREATE, UPDATE, and UNSET value ops applied
        // to the same entity + property id. We need to squash them down into a single
        // op so we can write to the db atomically using the final state of the ops.
        //
        // Ordering of these to-be-squashed ops matters. We use what the order is in
        // the edit.
        let original_count = value_ops.len();
        let squashed = squash_values(&value_ops);
        
        if squashed.len() < original_count {
            debug!(
                original_count,
                squashed_count = squashed.len(),
                dropped = original_count - squashed.len(),
                "Squashed duplicate value operations"
            );
        }

        let (created, deleted): (Vec<ValueOp>, Vec<ValueOp>) = squashed
            .into_iter()
            .partition(|op| matches!(op.change_type, ValueChangeType::SET));

        debug!(
            created_count = created.len(),
            deleted_count = deleted.len(),
            "Processed value operations"
        );

        return (created, deleted.iter().map(|op| op.id).collect());
    }
}

fn squash_values(value_ops: &Vec<ValueOp>) -> Vec<ValueOp> {
    let mut hash = HashMap::new();

    for op in value_ops {
        hash.insert(op.id, op.clone());
    }

    let result: Vec<_> = hash.into_values().collect();

    return result;
}

fn derive_value_id(entity_id: &Uuid, property_id: &Uuid, space_id: &Uuid) -> Uuid {
    let mut hasher = DefaultHasher::new();
    entity_id.hash(&mut hasher);
    property_id.hash(&mut hasher);
    space_id.hash(&mut hasher);
    let hash_value = hasher.finish();

    // Create a deterministic UUID from the hash
    let mut bytes = [0u8; 16];
    bytes[0..8].copy_from_slice(&hash_value.to_be_bytes());
    bytes[8..16].copy_from_slice(&hash_value.to_be_bytes());

    Uuid::from_bytes(bytes)
}

async fn value_op_from_op<C>(op: &Op, space_id: &Uuid, cache: &Arc<C>) -> Vec<ValueOp>
where
    C: ImmutableCache + Send + Sync + 'static,
{
    let mut values = Vec::new();

    if let Some(payload) = &op.payload {
        match payload {
            Payload::UpdateEntity(entity) => {
                let entity_id_bytes = id::transform_id_bytes(entity.id.clone());

                match entity_id_bytes {
                    Ok(entity_id_bytes) => {
                        let entity_id = Uuid::from_bytes(entity_id_bytes);
                        let mut skipped_values = 0;

                        for value in &entity.values {
                            let property_id_bytes = id::transform_id_bytes(value.property.clone());

                            if let Err(_) = property_id_bytes {
                                warn!(
                                    entity_id = %entity_id,
                                    property_bytes = ?value.property,
                                    "[Values][UpdateEntity] Could not transform Vec<u8> for property.id"
                                );
                                skipped_values += 1;
                                continue;
                            }

                            let property_id = Uuid::from_bytes(property_id_bytes.unwrap());

                            let (language, unit) = extract_options(&value.options);

                            let base_op = ValueOp {
                                id: derive_value_id(&entity_id, &property_id, space_id),
                                change_type: ValueChangeType::SET,
                                property_id,
                                entity_id,
                                space_id: space_id.clone(),
                                language,
                                unit,
                                string: None,
                                number: None,
                                boolean: None,
                                time: None,
                                point: None,
                            };

                            if let Some(populated_op) =
                                populate_value_fields_by_datatype(base_op, &value.value, cache)
                                    .await
                            {
                                values.push(populated_op);
                            } else {
                                skipped_values += 1;
                            }
                        }
                        
                        if skipped_values > 0 {
                            warn!(
                                entity_id = %entity_id,
                                skipped_count = skipped_values,
                                total_values = entity.values.len(),
                                "Some values were skipped during processing"
                            );
                        }
                    }
                    Err(_) => warn!(
                        entity_bytes = ?entity.id,
                        "[Values][UpdateEntity] Could not transform Vec<u8> for entity.id"
                    ),
                }
            }
            Payload::UnsetEntityValues(entity) => {
                let entity_id_bytes = id::transform_id_bytes(entity.id.clone());

                match entity_id_bytes {
                    Ok(entity_id_bytes) => {
                        let entity_id = Uuid::from_bytes(entity_id_bytes);

                        for property in &entity.properties {
                            let property_id_bytes =
                                id::transform_id_bytes(property.clone());

                            if let Err(_) = property_id_bytes {
                                warn!(
                                    entity_id = %entity_id,
                                    property_bytes = ?property,
                                    "[Values][UnsetEntityValues] Could not transform Vec<u8> for property id"
                                );
                                continue;
                            }

                            let property_id =
                                Uuid::from_bytes(property_id_bytes.unwrap());

                            values.push(ValueOp {
                                id: derive_value_id(&entity_id, &property_id, space_id),
                                change_type: ValueChangeType::DELETE,
                                property_id,
                                entity_id,
                                space_id: space_id.clone(),
                                language: None,
                                unit: None,
                                string: None,
                                number: None,
                                boolean: None,
                                time: None,
                                point: None,
                            });
                        }
                    },
                    Err(_) => warn!(
                        entity_bytes = ?entity.id,
                        "[Values][UnsetEntityValues] Could not transform Vec<u8> for entity.id"
                    )
                }
            }
            _ => {}
        };
    }

    return values;
}

/// Validates and populates the appropriate type-specific field based on data type.
/// Returns None if validation fails, indicating the value should be filtered out.
#[instrument(skip_all, fields(property_id = %base_op.property_id, entity_id = %base_op.entity_id))]
pub async fn populate_value_fields_by_datatype<C>(
    mut base_op: ValueOp,
    raw_value: &str,
    cache: &Arc<C>,
) -> Option<ValueOp>
where
    C: ImmutableCache + Send + Sync + 'static,
{
    // Only try to populate typed fields for SET operations with values
    if !matches!(base_op.change_type, ValueChangeType::SET) {
        return Some(base_op);
    }

    // Try to get the data type from cache
    if let Ok(data_type) = cache.get(&base_op.property_id).await {
        match validate_by_datatype(data_type, raw_value) {
            Ok(validated_value) => {
                // Set the appropriate field based on the validated value
                match validated_value {
                    ValidatedValue::Text(text) => {
                        // Even if it's a relation type, store as string
                        // Relations will be filtered out later
                        base_op.string = Some(text);
                    }
                    ValidatedValue::Number(num) => {
                        base_op.number = Some(num);
                    }
                    ValidatedValue::Checkbox(bool_val) => {
                        base_op.boolean = Some(bool_val);
                    }
                    ValidatedValue::Time(_) => {
                        base_op.time = Some(raw_value.to_string());
                    }
                    ValidatedValue::Point(_) => {
                        base_op.point = Some(raw_value.to_string());
                    }
                }

                return Some(base_op);
            }
            Err(error) => {
                // If validation fails, log the error and filter out the value
                warn!(
                    property_id = %base_op.property_id,
                    entity_id = %base_op.entity_id,
                    data_type = ?data_type,
                    value = raw_value,
                    error = %error,
                    "Value validation failed, filtering out"
                );
                return None;
            }
        }
    }
    // If property not found in cache, filter out the value
    else {
        warn!(
            property_id = %base_op.property_id,
            entity_id = %base_op.entity_id,
            "Property not found in cache, filtering out value"
        );
        return None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::properties_cache::{ImmutableCache, PropertiesCacheError};
    use crate::models::properties::DataType;
    use std::collections::HashMap;
    use tokio::runtime::Runtime;
    use tokio::sync::RwLock;

    // Mock cache implementation for testing
    #[derive(Default)]
    struct MockPropertiesCache {
        inner: Arc<RwLock<HashMap<Uuid, DataType>>>,
    }

    impl MockPropertiesCache {
        pub fn new() -> Self {
            Self {
                inner: Arc::new(RwLock::new(HashMap::new())),
            }
        }
    }

    #[async_trait::async_trait]
    impl ImmutableCache for MockPropertiesCache {
        async fn insert(&self, key: &Uuid, value: DataType) {
            let mut write = self.inner.write().await;
            write.insert(*key, value);
        }

        async fn get(&self, key: &Uuid) -> Result<DataType, PropertiesCacheError> {
            let read = self.inner.read().await;
            match read.get(key) {
                Some(value) => Ok(*value),
                None => Err(PropertiesCacheError::PropertyNotFoundError),
            }
        }
    }

    #[test]
    fn test_populate_value_fields_by_datatype() {
        let rt = Runtime::new().unwrap();
        let cache = Arc::new(MockPropertiesCache::new());
        let property_id = Uuid::new_v4();
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        // Set up cache with different property types
        rt.block_on(async {
            cache.insert(&property_id, DataType::Number).await;
        });

        // Test valid number
        let valid_number_op = ValueOp {
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
        };

        // Test invalid number
        let invalid_number_op = ValueOp {
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
        };

        // Validate results
        let valid_result = rt.block_on(populate_value_fields_by_datatype(
            valid_number_op,
            "123.45",
            &cache,
        ));
        let invalid_result = rt.block_on(populate_value_fields_by_datatype(
            invalid_number_op,
            "not-a-number",
            &cache,
        ));

        // Valid number should be returned with number field populated
        assert!(valid_result.is_some());
        let valid_op = valid_result.unwrap();
        assert!(valid_op.number.is_some());
        assert_eq!(valid_op.number.unwrap(), 123.45);

        // Invalid number should return None (filtered out)
        assert!(invalid_result.is_none());
    }
}

fn extract_options(options: &Option<wire::pb::grc20::Options>) -> (Option<String>, Option<String>) {
    if let Some(opts) = options {
        if let Some(value) = &opts.value {
            match value {
                options::Value::Text(text_opts) => {
                    let language = text_opts
                        .language
                        .as_ref()
                        .and_then(|lang| String::from_utf8(lang.clone()).ok());
                    (language, None)
                }
                options::Value::Number(number_opts) => {
                    let unit = number_opts.unit.as_ref().and_then(|unit_bytes| {
                        // If UTF-8 conversion fails, try treating it as raw UUID bytes
                        match id::transform_id_bytes(unit_bytes.clone()) {
                            Ok(uuid_bytes) => {
                                let uuid = Uuid::from_bytes(uuid_bytes);
                                return Some(uuid.to_string());
                            }
                            Err(_) => None,
                        }
                    });

                    (None, unit)
                }
            }
        } else {
            (None, None)
        }
    } else {
        (None, None)
    }
}
