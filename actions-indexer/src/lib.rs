//! Actions Indexer Library
//!
//! This library provides the core functionality for indexing blockchain actions,
//! including configuration management, error handling, and dependency injection.

pub mod config;
pub mod errors;

pub use config::Dependencies;
pub use errors::IndexingError;
