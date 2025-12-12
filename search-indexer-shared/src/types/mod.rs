//! This module defines the core data structures and types used across the search indexer.
//! It re-exports specific types like `EntityDocument`, `SearchQuery`, `SearchResult`, etc.

pub mod entity_document;
pub mod search_query;
pub mod search_result;

pub use entity_document::EntityDocument;
pub use search_query::{SearchQuery, SearchScope};
pub use search_result::{SearchResponse, SearchResult};

