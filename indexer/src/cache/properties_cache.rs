use std::{collections::HashMap, sync::Arc};

use tokio::sync::RwLock;

use crate::models::properties::DataType;

pub struct PropertiesCache {
    /// Represents the cache of property id -> data type. We store
    /// the data type as a u8 for smaller memory usage. Currently
    /// we store the property id as the full uuid representation
    /// rather than a more compressed representation.
    ///
    /// (byron: 2025-05-29): Since this cache is meant to be global
    /// and in-memory, we eventually want to use a more compressed
    /// representation as this will explode memory usage over time.
    /// For now it's fine.
    inner: Arc<RwLock<HashMap<String, u8>>>,
}

impl PropertiesCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
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
    async fn insert(&self, key: &String, value: DataType);
    async fn get(&self, key: &String) -> Result<u8, PropertiesCacheError>;
}

#[async_trait::async_trait]
impl ImmutableCache for PropertiesCache {
    async fn insert(&self, key: &String, value: DataType) {
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
        write.insert(key.clone(), DataType::to_int(&value));
    }

    async fn get(&self, key: &String) -> Result<u8, PropertiesCacheError> {
        let read = self.inner.read().await;

        return match read.get(key) {
            Some(value) => Ok(value.clone()),
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
        let key = "test_property".to_string();

        cache.insert(&key, DataType::Text).await;

        let result = cache.get(&key).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0); // Text maps to 0
    }

    #[tokio::test]
    async fn test_all_data_types_map_correctly() {
        let cache = PropertiesCache::new();

        // Test Text
        cache.insert(&"text_prop".to_string(), DataType::Text).await;
        assert_eq!(cache.get(&"text_prop".to_string()).await.unwrap(), 0);

        // Test Number
        cache
            .insert(&"number_prop".to_string(), DataType::Number)
            .await;
        assert_eq!(cache.get(&"number_prop".to_string()).await.unwrap(), 1);

        // Test Checkbox
        cache
            .insert(&"checkbox_prop".to_string(), DataType::Checkbox)
            .await;
        assert_eq!(cache.get(&"checkbox_prop".to_string()).await.unwrap(), 2);

        // Test Time
        cache.insert(&"time_prop".to_string(), DataType::Time).await;
        assert_eq!(cache.get(&"time_prop".to_string()).await.unwrap(), 3);

        // Test Point
        cache
            .insert(&"point_prop".to_string(), DataType::Point)
            .await;
        assert_eq!(cache.get(&"point_prop".to_string()).await.unwrap(), 4);

        // Test Relation
        cache
            .insert(&"relation_prop".to_string(), DataType::Relation)
            .await;
        assert_eq!(cache.get(&"relation_prop".to_string()).await.unwrap(), 5);
    }

    #[tokio::test]
    async fn test_immutability_duplicate_insert_ignored() {
        let cache = PropertiesCache::new();
        let key = "immutable_property".to_string();

        // Insert initial value
        cache.insert(&key, DataType::Text).await;
        let initial_value = cache.get(&key).await.unwrap();
        assert_eq!(initial_value, 0); // Text maps to 0

        // Attempt to overwrite with different data type
        cache.insert(&key, DataType::Number).await;

        // Value should remain unchanged (immutable)
        let final_value = cache.get(&key).await.unwrap();
        assert_eq!(final_value, 0); // Should still be Text (0), not Number (1)
    }

    #[tokio::test]
    async fn test_multiple_different_overwrites_ignored() {
        let cache = PropertiesCache::new();
        let key = "persistent_property".to_string();

        // Insert initial value
        cache.insert(&key, DataType::Checkbox).await;
        let initial_value = cache.get(&key).await.unwrap();
        assert_eq!(initial_value, 2); // Checkbox maps to 2

        // Attempt multiple overwrites with different data types
        cache.insert(&key, DataType::Text).await;
        cache.insert(&key, DataType::Number).await;
        cache.insert(&key, DataType::Time).await;
        cache.insert(&key, DataType::Point).await;
        cache.insert(&key, DataType::Relation).await;

        // Value should remain unchanged
        let final_value = cache.get(&key).await.unwrap();
        assert_eq!(final_value, 2); // Should still be Checkbox (2)
    }

    #[tokio::test]
    async fn test_get_nonexistent_property_returns_error() {
        let cache = PropertiesCache::new();
        let nonexistent_key = "does_not_exist".to_string();

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

        // Spawn multiple tasks that try to insert the same key with different values
        for i in 0..10 {
            let cache_clone = Arc::clone(&cache);
            let handle = tokio::spawn(async move {
                let key = "concurrent_test".to_string();
                let data_type = match i % 6 {
                    0 => DataType::Text,
                    1 => DataType::Number,
                    2 => DataType::Checkbox,
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
        let result = cache.get(&"concurrent_test".to_string()).await;
        assert!(result.is_ok());
        let value = result.unwrap();
        assert!(value <= 5); // Should be a valid DataType integer
    }

    #[tokio::test]
    async fn test_multiple_different_properties() {
        let cache = PropertiesCache::new();

        // Insert multiple different properties
        cache.insert(&"prop1".to_string(), DataType::Text).await;
        cache.insert(&"prop2".to_string(), DataType::Number).await;
        cache.insert(&"prop3".to_string(), DataType::Checkbox).await;

        // Verify all are stored correctly
        assert_eq!(cache.get(&"prop1".to_string()).await.unwrap(), 0);
        assert_eq!(cache.get(&"prop2".to_string()).await.unwrap(), 1);
        assert_eq!(cache.get(&"prop3".to_string()).await.unwrap(), 2);

        // Verify we can still get error for non-existent property
        assert!(cache.get(&"prop4".to_string()).await.is_err());
    }
}
