use std::{collections::HashMap, sync::Arc};

use tokio::sync::RwLock;

use super::{CacheBackend, CacheError, PreprocessedEdit};

pub struct KvCache {
    store: Arc<RwLock<HashMap<String, PreprocessedEdit>>>,
}

pub struct WriteCacheItem {
    pub uri: String,
    pub item: PreprocessedEdit,
}

impl KvCache {
    pub async fn new(seed_cache: Vec<WriteCacheItem>) -> Result<Self, CacheError> {
        let store = Arc::new(RwLock::new(HashMap::new()));

        for seed in seed_cache {
            let mut s = store.write().await;
            s.insert(seed.uri, seed.item);
        }

        return Ok(KvCache { store });
    }
}

#[async_trait::async_trait]
impl CacheBackend for KvCache {
    async fn get(&self, uri: &String) -> Result<PreprocessedEdit, CacheError> {
        let store = self.store.read().await;
        store.get(uri).cloned().ok_or(CacheError::NotFound)
    }
}
