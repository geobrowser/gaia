//! Graph data structures and algorithms
//!
//! This module contains the core graph types used throughout Atlas:
//! - `TreeNode`: Represents a node in a tree with edge metadata
//! - `TransitiveGraph`: Result of transitive closure computation
//! - `CanonicalGraph`: Result of canonical graph computation from a root
//! - `GraphState`: In-memory representation of the topology graph
//! - `DiffTracker`: Computes incremental diffs between canonical graphs
//! - `memory`: Functions for estimating heap memory usage

mod canonical;
mod diff;
mod hash;
pub mod memory;
mod state;
mod transitive;
mod tree;

pub use canonical::{CanonicalGraph, CanonicalProcessor};
pub use diff::{ChangeType, DiffTracker, GraphDiff, NodeChange, Position};
pub use hash::hash_tree;
pub use state::GraphState;
pub use transitive::{TransitiveGraph, TransitiveProcessor};
pub use tree::{EdgeType, TreeNode};
