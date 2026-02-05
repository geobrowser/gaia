//! # Search Indexer Shared
//!
//! This crate defines shared data structures and types used across the search indexer ecosystem.
//! It includes common definitions for entity documents used during indexing.

pub mod env;
pub mod types;

pub use env::{get_consumer_group_prefix, get_index_prefix};
pub use types::entity_document::{EntityDocument, TypeRelationEntry};
