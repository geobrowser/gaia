use std::{collections::HashMap, sync::Arc};

use tokio::sync::RwLock;

use crate::models::properties::DataType;

pub struct PropertiesCache {
    inner: Arc<RwLock<HashMap<String, u8>>>,
}

impl PropertiesCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

pub enum PropertiesCacheError {
    PropertyNotFoundError,
}

// How do we model the cache?
// How do we make it as small/packed as possible?
// We can store the value types as tiny ints
// Should be a KV cache
// How parallelizable should it be? If it's fast enough we don't have to lock
//  and just read properties serially

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
