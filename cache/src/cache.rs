use std::env;

use grc20::pb::ipfs::Edit;
use sqlx::{postgres::PgPoolOptions, Executor, Postgres};

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

// @TODO: How do we abstract to handle arbitrary storage mechanisms for the cache?
// e.g. we may want in-memory or a different db
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

    pub async fn insert(&self, item: &CacheItem) -> Result<(), CacheError> {
        let json_string = serde_json::to_value(&item.json)?;

        let query = sqlx::query!(
            "INSERT INTO ipfs_cache (uri, json, block, space, is_errored) VALUES ($1, $2, $3, $4, $5)",
            item.uri,
            json_string,
            item.block,
            item.space,
            item.is_errored
        );

        let result = self.connection.execute(query).await?;

        Ok(())
    }

    pub async fn has(&self, uri: &String) -> Result<bool, CacheError> {
        let maybe_exists = sqlx::query!(
            "SELECT EXISTS(SELECT 1 FROM ipfs_cache WHERE uri = $1)",
            uri
        )
        .fetch_one(&self.connection)
        .await?;

        Ok(maybe_exists.exists.unwrap_or(false))
    }
}

pub struct Cache {
    storage: Storage,
}

pub struct CacheItem {
    pub uri: String,
    pub json: Option<Edit>,
    pub block: String,
    pub space: String,
    pub is_errored: bool,
}

impl Cache {
    pub fn new(storage: Storage) -> Self {
        Cache { storage }
    }

    pub async fn put(&mut self, item: &CacheItem) -> Result<(), CacheError> {
        self.storage.insert(item).await?;

        Ok(())
    }

    pub async fn has(&mut self, uri: &String) -> Result<bool, CacheError> {
        let result = self.storage.has(uri).await?;
        Ok(result)
    }
}
