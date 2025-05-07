use std::{collections::HashSet, env};

use grc20::pb::ipfs::Edit;
use sqlx::{postgres::PgPoolOptions, Postgres};

use stream::utils::BlockMetadata;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Storage error: {0}")]
    Database(#[from] sqlx::Error),
}

#[async_trait::async_trait]
pub trait StorageBackend: Send + Sync {
    async fn insert_entities(&self, entities: &Vec<EntityItem>) -> Result<(), StorageError>;
}

pub struct PostgresStorage {
    pool: sqlx::Pool<Postgres>,
}

impl PostgresStorage {
    pub async fn new() -> Result<Self, StorageError> {
        let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");

        let database_url_static = database_url.as_str();

        let connection = PgPoolOptions::new()
            .max_connections(20)
            .connect(database_url_static)
            .await?;

        return Ok(PostgresStorage { pool });
    }
}

#[async_trait::async_trait]
impl StorageBackend for PostgresStorage {
    async fn insert_entities(&self, entities: &Vec<EntityItem>) -> Result<(), StorageError> {
        let ids: Vec<String> = entities.iter().map(|x| x.id.clone()).collect();
        let created_ats: Vec<String> = entities.iter().map(|x| x.created_at.clone()).collect();
        let created_at_blocks: Vec<String> = entities
            .iter()
            .map(|x| x.created_at_block.clone())
            .collect();
        let updated_ats: Vec<String> = entities.iter().map(|x| x.updated_at.clone()).collect();
        let updated_at_blocks: Vec<String> = entities
            .iter()
            .map(|x| x.updated_at_block.clone())
            .collect();

        // @TODO: How do we abstract sqlx?
        let result = sqlx::query!(
            r#"
            INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
            SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::text[], $5::text[])
            ON CONFLICT (id)
            DO UPDATE SET updated_at = EXCLUDED.updated_at, updated_at_block = EXCLUDED.updated_at_block
            "#,
            &ids,
            &created_ats,
            &created_at_blocks,
            &updated_ats,
            &updated_at_blocks
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

pub struct EntityItem {
    id: String,
    created_at: String,
    created_at_block: String,
    updated_at: String,
    updated_at_block: String,
}

pub struct EntitiesModel;

impl EntitiesModel {
    pub fn new() -> Self {
        Entity {}
    }

    pub fn map_edit_to_entities(edit: &Edit, block: &BlockMetadata) -> Vec<EntityItem> {
        let mut entities: Vec<EntityItem> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        for op in &edit.ops {
            match op.r#type {
                // SET_TRIPLE
                1 => {
                    if op.triple.is_some() {
                        let entity_id = op.triple.clone().unwrap().entity;

                        if !seen.contains(&entity_id) {
                            entities.push(EntityItem {
                                id: entity_id.clone(),
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(entity_id);
                        }
                    }
                }
                // DELETE_TRIPLE
                2 => {
                    if op.triple.is_some() {
                        let entity_id = op.triple.clone().unwrap().entity;

                        if !seen.contains(&entity_id) {
                            entities.push(EntityItem {
                                id: entity_id.clone(),
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(entity_id);
                        }
                    }
                }
                // CREATE_RELATION
                5 => {
                    if op.relation.is_some() {
                        let relation = op.relation.clone().unwrap();

                        let relation_id = relation.id.clone();
                        let from_id = relation.from_entity.clone();
                        let to_id = relation.to_entity.clone();
                        let type_id = relation.r#type.clone();

                        if !seen.contains(&relation_id) {
                            entities.push(EntityItem {
                                id: relation_id.clone(),
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(relation_id);
                        }

                        if !seen.contains(&from_id) {
                            entities.push(EntityItem {
                                id: from_id.clone(),
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(from_id);
                        }

                        if !seen.contains(&to_id) {
                            entities.push(EntityItem {
                                id: to_id.clone(),
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(to_id);
                        }

                        if !seen.contains(&type_id) {
                            entities.push(EntityItem {
                                id: type_id.clone(),
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(type_id);
                        }
                    }
                }
                // DELETE_RELATION
                6 => {
                    if op.relation.is_some() {
                        let relation = op.relation.clone().unwrap();
                        let relation_id = relation.id.clone();

                        if !seen.contains(&relation_id) {
                            entities.push(EntityItem {
                                id: relation_id.clone(),
                                created_at: block.timestamp.clone(),
                                created_at_block: block.block_number.to_string(),
                                updated_at: block.timestamp.clone(),
                                updated_at_block: block.block_number.to_string(),
                            });

                            seen.insert(relation_id);
                        }
                    }
                }
                _ => {}
            }
        }

        return entities;
    }
}

/**
use std::sync::Mutex;
use std::collections::HashMap;

pub struct InMemoryStorage {
    users: Mutex<HashMap<i32, User>>,
}

#[async_trait::async_trait]
impl StorageBackend for InMemoryStorage {
    async fn get_user(&self, user_id: i32) -> Option<User> {
        self.users.lock().unwrap().get(&user_id).cloned()
    }

    async fn insert_user(&self, user: User) -> anyhow::Result<()> {
        self.users.lock().unwrap().insert(user.id, user);
        Ok(())
    }
}*/
