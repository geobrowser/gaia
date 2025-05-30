use async_trait::async_trait;

pub mod postgres;

use thiserror::Error;

use crate::models::{
    entities::EntityItem,
    relations::{SetRelationItem, UnsetRelationItem, UpdateRelationItem},
    values::ValueOp,
};

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Storage error: {0}")]
    Database(#[from] sqlx::Error),
}

#[async_trait]
pub trait StorageBackend: Send + Sync {
    async fn insert_entities(&self, entities: &Vec<EntityItem>) -> Result<(), StorageError>;
    async fn insert_values(&self, properties: &Vec<ValueOp>) -> Result<(), StorageError>;
    async fn delete_values(
        &self,
        property_ids: &Vec<String>,
        space_id: &String,
    ) -> Result<(), StorageError>;
    async fn insert_relations(&self, relations: &Vec<SetRelationItem>) -> Result<(), StorageError>;
    async fn update_relations(
        &self,
        relations: &Vec<UpdateRelationItem>,
    ) -> Result<(), StorageError>;
    async fn unset_relation_fields(
        &self,
        relations: &Vec<UnsetRelationItem>,
    ) -> Result<(), StorageError>;
    async fn delete_relations(
        &self,
        relation_ids: &Vec<String>,
        space_id: &String,
    ) -> Result<(), StorageError>;
}
