//! Graph data structures and algorithms
//!
//! This module contains the core graph types used throughout Atlas:
//! - `TreeNode`: Represents a node in a tree with edge metadata
//! - `TransitiveGraph`: Result of transitive closure computation
//! - `CanonicalGraph`: Result of canonical graph computation from a root
//! - `GraphState`: In-memory representation of the topology graph
//! - `memory`: Functions for estimating heap memory usage

mod canonical;
mod hash;
pub mod memory;
mod state;
mod transitive;
mod tree;

pub use canonical::{CanonicalGraph, CanonicalProcessor};
pub use hash::{hash_tree, DefaultTreeHasher, TreeHasher};
pub use state::GraphState;
pub use transitive::{TransitiveCache, TransitiveGraph, TransitiveProcessor};
pub use tree::{EdgeType, TreeNode};
