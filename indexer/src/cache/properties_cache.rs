use std::{collections::HashMap, sync::Arc};

use tokio::sync::RwLock;
use uuid::Uuid;

use crate::models::properties::DataType;
use crate::storage::{postgres::PostgresStorage, StorageError};

pub struct PropertiesCache {
    /// Represents the cache of property id -> data type. We store
    /// the data type as a u8 for smaller memory usage. Internally
    /// we use binary UUID representation (16 bytes) instead of String
    /// representation (~36 bytes + overhead) for significant
    /// memory savings, while maintaining a string-based API.
    ///
    /// (byron: 2025-05-29): Since this cache is meant to be global
    /// and in-memory, we use the compressed representation of Uuid.
    /// Binary UUIDs provide ~55% memory reduction compared to strings.
    /// Note that over time we may want an even smaller representation,
    /// but it's difficult to get smaller without giving up uniqueness.
    ///
    /// Currently DataType enum has 6 variants. Rust will use a u8 to
    /// represent the data type, so it's safe to store the DataType enum
    /// directly.
    inner: Arc<RwLock<HashMap<Uuid, DataType>>>,
}

impl PropertiesCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn from_storage(storage: &PostgresStorage) -> Result<Self, StorageError> {
        let properties = storage.get_all_properties().await?;
        let mut cache_map = HashMap::new();
        
        for property in properties {
            cache_map.insert(property.id, property.data_type);
        }

        tracing::info!(
            "Initialized PropertiesCache from database with {} properties",
            cache_map.len()
        );

        Ok(Self {
            inner: Arc::new(RwLock::new(cache_map)),
        })
    }
}

#[derive(Debug)]
pub enum PropertiesCacheError {
    PropertyNotFoundError,
}

// One edgecase with immutability is that we _could_ have two properties
//  made in the same block with the same id but different data types.
//  This likely would only happen if done intentionally but we should
//  potentially protect against it.
#[async_trait::async_trait]
pub trait ImmutableCache {
    async fn insert(&self, key: &Uuid, value: DataType);
    async fn get(&self, key: &Uuid) -> Result<DataType, PropertiesCacheError>;
}

#[async_trait::async_trait]
impl ImmutableCache for PropertiesCache {
    async fn insert(&self, key: &Uuid, value: DataType) {
        {
            let read = self.inner.read().await;
            if let Some(_) = read.get(key) {
                tracing::info!(
                    "[PropertiesCache][Insert] Found invalid write to existing property id {:?} with value {:?}",
                    key,
                    &value
                );
                return;
            }
        }

        let mut write = self.inner.write().await;
        write.insert(*key, value.clone());
    }

    async fn get(&self, key: &Uuid) -> Result<DataType, PropertiesCacheError> {
        let read = self.inner.read().await;

        return match read.get(key) {
            Some(value) => Ok(*value),
            None => Err(PropertiesCacheError::PropertyNotFoundError),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::properties::DataType;

    #[tokio::test]
    async fn test_insert_and_get_property() {
        let cache = PropertiesCache::new();
        let key = Uuid::new_v4();

        cache.insert(&key, DataType::String).await;

        let result = cache.get(&key).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), DataType::String);
    }

    #[tokio::test]
    async fn test_all_data_types_map_correctly() {
        let cache = PropertiesCache::new();

        // Test Text
        let text_uuid = Uuid::new_v4();
        cache.insert(&text_uuid, DataType::String).await;
        assert_eq!(cache.get(&text_uuid).await.unwrap(), DataType::String);

        // Test Number
        let number_uuid = Uuid::new_v4();
        cache.insert(&number_uuid, DataType::Number).await;
        assert_eq!(cache.get(&number_uuid).await.unwrap(), DataType::Number);

        // Test Checkbox
        let checkbox_uuid = Uuid::new_v4();
        cache.insert(&checkbox_uuid, DataType::Boolean).await;
        assert_eq!(cache.get(&checkbox_uuid).await.unwrap(), DataType::Boolean);

        // Test Time
        let time_uuid = Uuid::new_v4();
        cache.insert(&time_uuid, DataType::Time).await;
        assert_eq!(cache.get(&time_uuid).await.unwrap(), DataType::Time);

        // Test Point
        let point_uuid = Uuid::new_v4();
        cache.insert(&point_uuid, DataType::Point).await;
        assert_eq!(cache.get(&point_uuid).await.unwrap(), DataType::Point);

        // Test Relation
        let relation_uuid = Uuid::new_v4();
        cache.insert(&relation_uuid, DataType::Relation).await;
        assert_eq!(cache.get(&relation_uuid).await.unwrap(), DataType::Relation);
    }

    #[tokio::test]
    async fn test_immutability_duplicate_insert_ignored() {
        let cache = PropertiesCache::new();
        let key = Uuid::new_v4();

        // Insert initial value
        cache.insert(&key, DataType::String).await;
        let initial_value = cache.get(&key).await.unwrap();
        assert_eq!(initial_value, DataType::String);

        // Attempt to overwrite with different data type
        cache.insert(&key, DataType::Number).await;

        // Value should remain unchanged (immutable)
        let final_value = cache.get(&key).await.unwrap();
        assert_eq!(final_value, DataType::String); // Should still be Text, not Number
    }

    #[tokio::test]
    async fn test_multiple_different_overwrites_ignored() {
        let cache = PropertiesCache::new();
        let key = Uuid::new_v4();

        // Insert initial value
        cache.insert(&key, DataType::Boolean).await;
        let initial_value = cache.get(&key).await.unwrap();
        assert_eq!(initial_value, DataType::Boolean);

        // Attempt multiple overwrites with different data types
        cache.insert(&key, DataType::String).await;
        cache.insert(&key, DataType::Number).await;
        cache.insert(&key, DataType::Time).await;
        cache.insert(&key, DataType::Point).await;
        cache.insert(&key, DataType::Relation).await;

        // Value should remain unchanged
        let final_value = cache.get(&key).await.unwrap();
        assert_eq!(final_value, DataType::Boolean); // Should still be Checkbox
    }

    #[tokio::test]
    async fn test_get_nonexistent_property_returns_error() {
        let cache = PropertiesCache::new();
        let nonexistent_key = Uuid::new_v4();

        let result = cache.get(&nonexistent_key).await;
        assert!(result.is_err());

        match result {
            Err(PropertiesCacheError::PropertyNotFoundError) => {
                // Expected error type
            }
            _ => panic!("Expected PropertyNotFoundError"),
        }
    }

    #[tokio::test]
    async fn test_concurrent_access() {
        let cache = Arc::new(PropertiesCache::new());
        let mut handles = vec![];
        let key = Uuid::new_v4();

        // Spawn multiple tasks that try to insert the same key with different values
        for i in 0..10 {
            let cache_clone = Arc::clone(&cache);
            let handle = tokio::spawn(async move {
                let data_type = match i % 6 {
                    0 => DataType::String,
                    1 => DataType::Number,
                    2 => DataType::Boolean,
                    3 => DataType::Time,
                    4 => DataType::Point,
                    _ => DataType::Relation,
                };
                cache_clone.insert(&key, data_type).await;
            });
            handles.push(handle);
        }

        // Wait for all tasks to complete
        for handle in handles {
            handle.await.unwrap();
        }

        // Only one value should have been stored (whichever won the race)
        let result = cache.get(&key).await;
        assert!(result.is_ok());
        let value = result.unwrap();
        // Should be one of the valid DataType variants
        assert!(matches!(
            value,
            DataType::String
                | DataType::Number
                | DataType::Boolean
                | DataType::Time
                | DataType::Point
                | DataType::Relation
        ));
    }

    #[tokio::test]
    async fn test_multiple_different_properties() {
        let cache = PropertiesCache::new();

        // Insert multiple different properties
        let prop1 = Uuid::new_v4();
        let prop2 = Uuid::new_v4();
        let prop3 = Uuid::new_v4();

        cache.insert(&prop1, DataType::String).await;
        cache.insert(&prop2, DataType::Number).await;
        cache.insert(&prop3, DataType::Boolean).await;

        // Verify all are stored correctly
        assert_eq!(cache.get(&prop1).await.unwrap(), DataType::String);
        assert_eq!(cache.get(&prop2).await.unwrap(), DataType::Number);
        assert_eq!(cache.get(&prop3).await.unwrap(), DataType::Boolean);

        // Verify we can still get error for non-existent property
        let prop4 = Uuid::new_v4();
        assert!(cache.get(&prop4).await.is_err());
    }
}
