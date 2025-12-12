//! OpenSearch implementation of the search index provider.
//!
//! This module provides a concrete implementation of `SearchIndexProvider`
//! using OpenSearch as the backend.

mod index_config;
mod provider;

pub use index_config::{get_index_settings, get_versioned_index_name, IndexConfig, INDEX_NAME};
pub use provider::OpenSearchProvider;
