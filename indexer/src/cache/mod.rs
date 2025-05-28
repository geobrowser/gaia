use async_trait::async_trait;
pub mod kv;
pub mod postgres;

use grc20::pb::ipfs::Edit;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CacheError {
    #[error("Cache error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Cache error")]
    NotFound,

    #[error("Cache error: {0}")]
    DeserializeError(#[from] serde_json::Error),
}

#[derive(Clone, Debug)]
pub struct PreprocessedEdit {
    pub edit: Option<Edit>,
    pub is_errored: bool,
    pub space_id: String,
}

#[async_trait]
pub trait CacheBackend: Send + Sync {
    async fn get(&self, uri: &String) -> Result<PreprocessedEdit, CacheError>;
}
