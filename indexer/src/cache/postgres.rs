use std::env;

use grc20::pb::ipfsv2::Edit;
use sqlx::{postgres::PgPoolOptions, Postgres};

use super::{CacheBackend, CacheError, PreprocessedEdit};

pub struct PostgresCache {
    pool: sqlx::Pool<Postgres>,
}

impl PostgresCache {
    pub async fn new() -> Result<Self, CacheError> {
        let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");

        let database_url_static = database_url.as_str();

        let pool = PgPoolOptions::new()
            .max_connections(20)
            .connect(database_url_static)
            .await?;

        return Ok(PostgresCache { pool });
    }
}

#[async_trait::async_trait]
impl CacheBackend for PostgresCache {
    async fn get(&self, uri: &String) -> Result<PreprocessedEdit, CacheError> {
        let query = sqlx::query!(
            "SELECT json, is_errored, space FROM ipfs_cache WHERE uri = $1",
            uri
        )
        .fetch_one(&self.pool)
        .await?;

        if query.is_errored {
            return Ok(PreprocessedEdit {
                edit: None,
                is_errored: true,
                space_id: query.space,
            });
        }

        let json = query.json.unwrap();
        let edit = serde_json::from_value::<Edit>(json)?;

        Ok(PreprocessedEdit {
            edit: Some(edit),
            is_errored: false,
            space_id: query.space,
        })
    }
}
