use async_trait::async_trait;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

use super::{entities::EntityItem, StorageBackend, StorageError};

pub struct KvStorage {
    store: Arc<Mutex<HashMap<String, EntityItem>>>,
}

impl KvStorage {
    pub fn new() -> Self {
        Self {
            store: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn get(&self, key: &String) -> Option<EntityItem> {
        self.store.lock().await.get(key).cloned()
    }
}

#[async_trait]
impl StorageBackend for KvStorage {
    async fn insert_entities(&self, entities: &Vec<EntityItem>) -> Result<(), StorageError> {
        for entity in entities {
            let mut store = self.store.lock().await;
            store.insert(entity.id.clone(), entity.clone());
        }

        Ok(())
    }
}
