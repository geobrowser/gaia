//! PostgreSQL implementation of the cursor repository.
//!
//! Stores cursor state in a `meta` table to enable indexer resumption after restarts.

use crate::CursorRepository;
use crate::errors::CursorRepositoryError;
use async_trait::async_trait;

/// PostgreSQL-backed cursor repository.
///
/// Persists indexer cursors in a `meta` table with upsert operations for atomic updates.
pub struct PostgresCursorRepository {
    /// PostgreSQL connection pool
    pool: sqlx::PgPool,
}

impl PostgresCursorRepository {
    /// Creates a new PostgreSQL cursor repository instance.
    ///
    /// # Arguments
    ///
    /// * `pool` - Configured PostgreSQL connection pool with required schema (meta table)
    ///
    /// # Returns
    ///
    /// * `Ok(PostgresCursorRepository)` - Ready-to-use repository instance
    /// * `Err(CursorRepositoryError)` - Future validation errors (currently always succeeds)
    pub async fn new(pool: sqlx::PgPool) -> Result<Self, CursorRepositoryError> {
        Ok(Self { pool })
    }
}

#[async_trait]
impl CursorRepository for PostgresCursorRepository {
    async fn get_cursor(&self, id: &str) -> Result<Option<String>, CursorRepositoryError> {
        let result = sqlx::query!("SELECT cursor FROM meta WHERE id = $1", id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(result.map(|row| row.cursor))
    }

    async fn save_cursor(
        &self,
        id: &str,
        cursor: &str,
        block_number: &i64,
    ) -> Result<(), CursorRepositoryError> {
        sqlx::query!(
            "INSERT INTO meta (id, cursor, block_number) VALUES ($1, $2, $3) ON CONFLICT (id) DO UPDATE SET cursor = $2, block_number = $3",
            id,
            cursor,
            block_number.to_string()
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
