//! # Search Indexer Repository
//!
//! This crate provides traits and implementations for interacting with the
//! search index. It includes definitions for errors, interfaces, and a
//! concrete implementation for OpenSearch.

pub mod config;
pub mod errors;
pub mod interfaces;
pub mod opensearch;
pub mod service;
pub mod types;
pub mod utils;

pub use config::SearchIndexServiceConfig;
pub use errors::SearchIndexError;
pub use interfaces::SearchIndexProvider;
pub use opensearch::OpenSearchProvider;
pub use service::SearchIndexService;
pub use types::{
    BatchOperationResult, BatchOperationSummary, DeleteEntityRequest, UnsetEntityPropertiesRequest,
    UpdateEntityRequest,
};
pub use utils::parse_entity_and_space_ids;
