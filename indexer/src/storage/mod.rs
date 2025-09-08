use async_trait::async_trait;
use sqlx::Postgres;
use uuid::Uuid;

pub mod postgres;

use thiserror::Error;

use crate::models::{
    entities::EntityItem,
    membership::{EditorItem, MemberItem},
    properties::PropertyItem,
    relations::{SetRelationItem, UnsetRelationItem, UpdateRelationItem},
    spaces::SpaceItem,
    subspaces::SubspaceItem,
    values::ValueOp,
};

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Storage error: {0}")]
    Database(#[from] sqlx::Error),
}

#[async_trait]
pub trait StorageBackend: Send + Sync {
    fn get_pool(&self) -> &sqlx::Pool<Postgres>;
    async fn insert_entities(
        &self,
        entities: &Vec<EntityItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError>;
    async fn insert_values(
        &self,
        properties: &Vec<ValueOp>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError>;
    async fn delete_values(
        &self,
        value_ids: &Vec<Uuid>,
        space_id: &Uuid,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError>;
    async fn insert_relations(
        &self,
        relations: &Vec<SetRelationItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError>;
    async fn update_relations(
        &self,
        relations: &Vec<UpdateRelationItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError>;
    async fn unset_relation_fields(
        &self,
        relations: &Vec<UnsetRelationItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError>;
    async fn delete_relations(
        &self,
        relation_ids: &Vec<Uuid>,
        space_id: &Uuid,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError>;
    async fn insert_properties(
        &self,
        properties: &Vec<PropertyItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError>;
    async fn insert_spaces(
        &self,
        spaces: &Vec<SpaceItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError>;
    async fn insert_members(
        &self,
        members: &Vec<MemberItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError>;
    async fn remove_members(
        &self,
        members: &Vec<MemberItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError>;
    async fn insert_editors(
        &self,
        editors: &Vec<EditorItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError>;
    async fn remove_editors(
        &self,
        editors: &Vec<EditorItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError>;
    async fn insert_subspaces(
        &self,
        subspaces: &Vec<SubspaceItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError>;
    async fn remove_subspaces(
        &self,
        subspaces: &Vec<SubspaceItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError>;
}
