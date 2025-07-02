use async_trait::async_trait;
use uuid::Uuid;

pub mod postgres;

use thiserror::Error;

use crate::models::{
    entities::EntityItem,
    membership::{EditorItem, MemberItem},
    properties::PropertyItem,
    relations::{SetRelationItem, UnsetRelationItem, UpdateRelationItem},
    spaces::SpaceItem,
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
        value_ids: &Vec<Uuid>,
        space_id: &Uuid,
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
        relation_ids: &Vec<Uuid>,
        space_id: &Uuid,
    ) -> Result<(), StorageError>;
    async fn insert_properties(&self, properties: &Vec<PropertyItem>) -> Result<(), StorageError>;
    async fn insert_spaces(&self, spaces: &Vec<SpaceItem>) -> Result<(), StorageError>;
    async fn insert_members(&self, members: &Vec<MemberItem>) -> Result<(), StorageError>;
    async fn remove_members(&self, members: &Vec<MemberItem>) -> Result<(), StorageError>;
    async fn insert_editors(&self, editors: &Vec<EditorItem>) -> Result<(), StorageError>;
    async fn remove_editors(&self, editors: &Vec<EditorItem>) -> Result<(), StorageError>;
}
