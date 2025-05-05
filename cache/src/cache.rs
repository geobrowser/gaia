use std::env;

use grc20::pb::ipfs::Edit;
use sqlx::{
    postgres::{PgPoolOptions, PgQueryResult},
    Executor, FromRow, Postgres,
};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum CacheError {
    #[error("Cache error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Serialize error: {0}")]
    SerializeError(#[from] serde_json::Error),
}

pub struct Storage {
    connection: sqlx::Pool<Postgres>,
}

impl Storage {
    pub async fn new() -> Result<Self, CacheError> {
        let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");

        let database_url_static = database_url.as_str();

        let connection = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url_static)
            .await?;

        return Ok(Storage { connection });
    }

    pub async fn insert(&self, item: &CacheItem) -> Result<PgQueryResult, CacheError> {
        let json_string = serde_json::to_value(&item.json)?;

        let query = sqlx::query!(
            "INSERT INTO ipfs_cache (uri, json, block, space) VALUES ($1, $2, $3, $4)",
            item.uri,
            json_string,
            item.block,
            item.space
        );

        let result = self.connection.execute(query).await?;

        println!("Result of insert {:?}", result.rows_affected());

        Ok(result)
    }
}

pub struct Cache {
    storage: Storage,
}

pub struct CacheItem {
    pub uri: String,
    pub json: Edit,
    pub block: String,
    pub space: String,
}

impl Cache {
    pub fn new(storage: Storage) -> Self {
        Cache { storage }
    }

    pub async fn put(&mut self, item: &CacheItem) -> Result<(), CacheError> {
        self.storage.insert(item).await?;

        Ok(())
    }
}
