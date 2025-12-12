//! # Search Indexer Shared
//!
//! This crate defines shared data structures and types used across the search indexer ecosystem.
//! It includes common definitions for entity documents, search queries, and search results.

pub mod types;

pub use types::entity_document::EntityDocument;
pub use types::search_query::{SearchQuery, SearchScope};
pub use types::search_result::{SearchResponse, SearchResult};

