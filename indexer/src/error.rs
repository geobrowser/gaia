use prost::DecodeError;
use thiserror::Error;
use tokio::task::JoinError;

use crate::{cache::CacheError, storage::StorageError};

#[derive(Error, Debug)]
pub enum IndexingError {
    #[error("Indexing error: {0}")]
    StorageError(#[from] StorageError),

    #[error("Indexing error: {0}")]
    CacheError(#[from] CacheError),

    #[error("Indexing error: {0}")]
    DecodeError(#[from] DecodeError),

    #[error("Indexing error: {0}")]
    TaskError(#[from] JoinError),

    #[error("Indexing error: {0}")]
    SqlxError(#[from] sqlx::Error),
}
