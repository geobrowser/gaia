use async_trait::async_trait;

pub mod postgres;

use thiserror::Error;

use crate::models::{entities::EntityItem, triples::TripleOp};

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Storage error: {0}")]
    Database(#[from] sqlx::Error),
}

#[async_trait]
pub trait StorageBackend: Send + Sync {
    async fn insert_entities(&self, entities: &Vec<EntityItem>) -> Result<(), StorageError>;
    async fn insert_triples(&self, triples: &Vec<TripleOp>) -> Result<(), StorageError>;
    async fn delete_triples(&self, triple_ids: &Vec<String>) -> Result<(), StorageError>;
}
