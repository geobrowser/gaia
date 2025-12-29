//! Error types for the loader module of the Actions Indexer Pipeline.
//! Defines specific errors that can occur during the loading and persistence
//! of processed action data.
use actions_indexer_repository::ActionsRepositoryError;
use actions_indexer_repository::CursorRepositoryError;
use thiserror::Error;

/// Represents errors that can occur within the action loader.
///
/// This enum consolidates various error conditions specific to the loading
/// process, including errors propagated from the actions repository.
#[derive(Debug, Error)]
pub enum LoaderError {
    #[error("Actions repository error: {0}")]
    ActionsRepository(#[from] ActionsRepositoryError),
    #[error("Cursor repository error: {0}")]
    CursorRepository(#[from] CursorRepositoryError),
}
