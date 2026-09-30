/// Transient errors are retried with backoff and never skipped past (a stalled indexer loses
/// freshness, never correctness). Fatal errors end the process: a descriptor that does not match
/// the slot means no vector from this run may be written.
#[derive(Debug, thiserror::Error)]
pub enum IndexerError {
    #[error("transient: {0}")]
    Transient(String),
    #[error("fatal: {0}")]
    Fatal(String),
}

pub type Result<T> = std::result::Result<T, IndexerError>;

impl IndexerError {
    pub fn transient(e: impl std::fmt::Display) -> Self {
        Self::Transient(e.to_string())
    }
    pub fn fatal(e: impl std::fmt::Display) -> Self {
        Self::Fatal(e.to_string())
    }
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::Fatal(_))
    }
}
