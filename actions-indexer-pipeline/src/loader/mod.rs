//! This module defines the `ActionsLoader` struct responsible for persisting
//! processed action changesets to a repository.
//! It acts as an interface between the processing pipeline and the data storage.
pub use crate::errors::LoaderError;
pub use actions_indexer_repository::PostgresActionsRepository;
pub use actions_indexer_repository::{ActionsRepository, CursorRepository};
pub use actions_indexer_shared::types::Changeset;
use std::sync::Arc;

/// `ActionsLoader` is responsible for loading and persisting changesets of actions.
///
/// It utilizes an `ActionsRepository` to interact with the underlying data store,
/// ensuring that processed action data is correctly stored.
pub struct ActionsLoader {
    pub actions_repository: Arc<dyn ActionsRepository>,
    pub cursor_repository: Arc<dyn CursorRepository>,
}

impl ActionsLoader {
    /// Creates a new `ActionsLoader` instance.
    ///
    /// # Arguments
    ///
    /// * `actions_repository` - An `Arc` (Atomically Reference Counted) trait object
    ///   that implements `ActionsRepository`, providing the interface for data persistence.
    /// * `cursor_repository` - An `Arc` (Atomically Reference Counted) trait object
    ///   that implements `CursorRepository`, providing the interface for block data persistence.
    ///
    /// # Returns
    ///
    /// A new `ActionsLoader` instance.
    pub fn new(
        actions_repository: Arc<dyn ActionsRepository>,
        cursor_repository: Arc<dyn CursorRepository>,
    ) -> Self {
        Self {
            actions_repository,
            cursor_repository,
        }
    }

    /// Persists a given `Changeset` to the actions repository.
    ///
    /// This asynchronous method takes a reference to a `Changeset` and delegates
    /// the persistence operation to the internal `actions_repository`.
    ///
    /// # Arguments
    ///
    /// * `changeset` - A reference to the `Changeset` to be persisted.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or a `LoaderError` if the persistence fails.
    pub async fn persist_changeset<'a>(
        &self,
        changeset: &'a Changeset<'a>,
    ) -> Result<(), LoaderError> {
        self.actions_repository.persist_changeset(changeset).await?;
        Ok(())
    }

    /// Persists the latest processed block number to the block repository.
    ///
    /// This asynchronous method takes a reference to a `block_number` and delegates
    /// the persistence operation to the internal `cursor_repository`.
    ///
    /// # Arguments
    ///
    /// * `id` - The id of the indexer.
    /// * `block_number` - The block number to persist.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or a `LoaderError` if the persistence fails.
    pub async fn persist_latest_processed_block_number(
        &self,
        id: &str,
        cursor: &str,
        block_number: &i64,
    ) -> Result<(), LoaderError> {
        self.cursor_repository
            .save_cursor(id, cursor, block_number)
            .await
            .map_err(LoaderError::from)?;
        Ok(())
    }
}
