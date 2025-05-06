use grc20::pb::ipfs::Edit;
use sqlx::{postgres::PgPoolOptions, Postgres};
use std::env;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CacheError {
    #[error("Cache error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Cache error: {0}")]
    DeserializeError(#[from] serde_json::Error),
}

pub struct Cache {
    connection: sqlx::Pool<Postgres>,
}

pub struct ReadCacheItem {
    pub edit: Option<Edit>,
    pub is_errored: bool,
}

impl Cache {
    pub async fn new() -> Result<Self, CacheError> {
        let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");

        let database_url_static = database_url.as_str();

        let connection = PgPoolOptions::new()
            .max_connections(20)
            .connect(database_url_static)
            .await?;

        return Ok(Cache { connection });
    }

    pub async fn get(&self, uri: &String) -> Result<ReadCacheItem, CacheError> {
        let query = sqlx::query!(
            "SELECT json, is_errored FROM ipfs_cache WHERE uri = $1",
            uri
        )
        .fetch_one(&self.connection)
        .await?;

        if query.is_errored {
            return Ok(ReadCacheItem {
                edit: None,
                is_errored: true,
            });
        }

        let json = query.json.unwrap();
        let edit = serde_json::from_value::<Edit>(json)?;

        Ok(ReadCacheItem {
            edit: Some(edit),
            is_errored: false,
        })
    }
}
