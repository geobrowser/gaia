use crate::errors::CursorRepositoryError;

/// Trait for interacting with the cursor repository.
///
/// This trait provides a clean abstraction over the underlying data store for the actions indexer system. It handles the retrieval and persistence of the cursor.
#[async_trait::async_trait]
pub trait CursorRepository: Send + Sync {
    /// Retrieves the cursor for a given ID.
    ///
    /// # Arguments
    ///
    /// * `id` - The ID to retrieve the cursor for.
    ///
    /// # Returns
    ///
    /// A `Result` containing the cursor if it exists, or `None` if it does not.
    async fn get_cursor(&self, id: &str) -> Result<Option<String>, CursorRepositoryError>;

    /// Saves the cursor for a given ID.
    ///
    /// # Arguments
    ///
    /// * `id` - The ID to save the cursor for.
    /// * `cursor` - The cursor to save.
    /// * `block_number` - The block number to save the cursor for.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or failure.
    async fn save_cursor(
        &self,
        id: &str,
        cursor: &str,
        block_number: &i64,
    ) -> Result<(), CursorRepositoryError>;
}
