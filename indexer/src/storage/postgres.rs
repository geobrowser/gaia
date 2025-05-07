use std::env;

use sqlx::{postgres::PgPoolOptions, Postgres};

use super::{EntityItem, StorageBackend, StorageError};

pub struct PostgresStorage {
    pool: sqlx::Pool<Postgres>,
}

impl PostgresStorage {
    pub async fn new() -> Result<Self, StorageError> {
        let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");

        let database_url_static = database_url.as_str();

        let pool = PgPoolOptions::new()
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
