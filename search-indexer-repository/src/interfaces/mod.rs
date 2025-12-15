//! Interface definitions for the search index provider.
//!
//! This module defines the abstract `SearchIndexProvider` trait that allows
//! for dependency injection and swappable search backend implementations.

mod search_index_provider;

pub use search_index_provider::SearchIndexProvider;
