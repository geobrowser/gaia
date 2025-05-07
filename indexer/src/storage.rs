use std::{any::Any, collections::HashSet, env};

use grc20::pb::ipfs::Edit;
use sqlx::{postgres::PgPoolOptions, Postgres};

use stream::utils::BlockMetadata;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CacheError {
    #[error("Cache error: {0}")]
    Database(#[from] sqlx::Error),
}

pub struct Storage {
    connection: sqlx::Pool<Postgres>,
}

impl Storage {
    pub async fn new() -> Result<Self, CacheError> {
        let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");

        let database_url_static = database_url.as_str();

        let connection = PgPoolOptions::new()
            .max_connections(20)
            .connect(database_url_static)
            .await?;

        return Ok(Storage { connection });
    }
}

pub struct EntityItem {
    id: String,
    created_at: String,
    created_at_block: String,
    updated_at: String,
    updated_at_block: String,
}

pub struct EntityStorage {
    storage: Storage,
}

#[derive(Error, Debug)]
pub enum EntityStorageError {
    #[error("Entity storage error: {0}")]
    Database(#[from] sqlx::Error),
}

impl EntityStorage {
    pub fn new(storage: Storage) -> Self {
        return EntityStorage { storage };
    }

    pub fn map_edit_to_entities(&self, edit: &Edit, block: &BlockMetadata) -> Vec<EntityItem> {
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

    pub async fn insert(&self, entities: &Vec<EntityItem>) -> Result<(), EntityStorageError> {
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
        .execute(&self.storage.connection)
        .await?;

        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum TripleStorageError {
    #[error("Triple storage error: {0}")]
    Database(#[from] sqlx::Error),
}

pub struct TripleStorage {
    storage: Storage,
}

impl TripleStorage {
    pub fn new(storage: Storage) -> Self {
        return TripleStorage { storage };
    }

    pub fn map_edit_to_triples(&self, edit: &Edit, block: &BlockMetadata) -> Vec<EntityItem> {
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

    pub async fn insert(&self, entities: &Vec<EntityItem>) -> Result<(), EntityStorageError> {
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
        .execute(&self.storage.connection)
        .await?;

        Ok(())
    }
}
