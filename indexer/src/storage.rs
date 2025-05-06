use std::env;

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
    #[error("Cache error: {0}")]
    Database(#[from] sqlx::Error),
}

impl EntityStorage {
    pub fn new(storage: Storage) -> Self {
        return EntityStorage { storage };
    }

    pub fn map_edit_to_entity_items(&self, edit: Edit, block: &BlockMetadata) -> Vec<EntityItem> {
        let mut entities: Vec<EntityItem> = Vec::new();

        for op in &edit.ops {
            match op.r#type {
                // SET_TRIPLE
                1 => {
                    if op.triple.is_some() {
                        entities.push(EntityItem {
                            id: op.triple.clone().unwrap().entity,
                            created_at: block.timestamp.clone(),
                            created_at_block: block.block_number.to_string(),
                            updated_at: block.timestamp.clone(),
                            updated_at_block: block.block_number.to_string(),
                        })
                    }
                }
                _ => {}
            }
        }

        if entities.len() > 0 {
            println!("More than one entity");
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
            "INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
             SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::text[], $5::text[])",
            &ids,
            &created_ats,
            &created_at_blocks,
            &updated_ats,
            &updated_at_blocks
        )
        .execute(&self.storage.connection)
        .await?;

        println!("Successfully wrote entities {}", result.rows_affected());

        Ok(())
    }
}
