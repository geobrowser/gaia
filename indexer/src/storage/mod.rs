use async_trait::async_trait;

pub mod postgres;

use thiserror::Error;

use crate::models::{entities::EntityItem, properties::PropertyOp};

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Storage error: {0}")]
    Database(#[from] sqlx::Error),
}

#[async_trait]
pub trait StorageBackend: Send + Sync {
    async fn insert_entities(&self, entities: &Vec<EntityItem>) -> Result<(), StorageError>;
    async fn insert_properties(&self, properties: &Vec<PropertyOp>) -> Result<(), StorageError>;
    async fn delete_properties(&self, property_ids: &Vec<String>) -> Result<(), StorageError>;
}
