//! Topology module for maintaining the canonical space graph.
//!
//! Consumes `CanonicalGraphDiff` messages from Atlas and maintains an in-memory
//! graph state used for:
//! - Determining `in_canonical_graph` status for entity documents
//! - Serving subspace queries for SPACE scope expansion in the Search API

pub mod persistence;
pub mod state;

pub use state::{CanonicalGraphState, CanonicalityChange};
