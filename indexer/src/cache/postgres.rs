use std::env;

use sqlx::{postgres::PgPoolOptions, Postgres, Row};
use uuid::Uuid;
use wire::pb::grc20::Edit;

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

    pub fn get_pool(&self) -> &sqlx::Pool<Postgres> {
        &self.pool
    }
}

#[async_trait::async_trait]
impl CacheBackend for PostgresCache {
    async fn get(&self, uri: &String) -> Result<PreprocessedEdit, CacheError> {
        let row = sqlx::query("SELECT json, is_errored, space FROM ipfs_cache WHERE uri = $1")
            .bind(uri)
            .fetch_one(&self.pool)
            .await?;

        let is_errored: bool = row.get("is_errored");
        let space: Uuid = row.get("space");

        if is_errored {
            return Ok(PreprocessedEdit {
                edit: None,
                is_errored: true,
                space_id: space,
                cid: uri.clone(),
            });
        }

        let json: Option<serde_json::Value> = row.get("json");
        let json = json.unwrap();
        let edit = serde_json::from_value::<Edit>(json)?;

        Ok(PreprocessedEdit {
            edit: Some(edit),
            is_errored: false,
            space_id: space,
            cid: uri.clone(),
        })
    }
}
