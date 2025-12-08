//! Atlas - Graph Storage System with Dynamic Group Resolution
//!
//! A multi-graph tracking system that supports:
//! - Multiple graph views (Global, Local, Transitive DAG, Canonical)
//! - Group abstractions with dynamic resolution at query time
//! - Trust model based on reachability from root

pub mod convert;
pub mod events;
pub mod graph;
pub mod kafka;

// Re-export the internal mock_substream for backwards compatibility
// TODO: Remove this once all code migrates to the shared mock_substream crate
#[deprecated(note = "Use the shared mock_substream crate instead")]
pub mod mock_substream;
