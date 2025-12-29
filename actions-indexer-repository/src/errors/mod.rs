//! Error types for the actions indexer repository.
//! Consolidates and re-exports error types related to actions repository operations.
mod actions;
mod cursor_repository;

pub use actions::ActionsRepositoryError;
pub use cursor_repository::CursorRepositoryError;
