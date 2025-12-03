//! Atlas - Graph Storage System with Dynamic Group Resolution
//!
//! A multi-graph tracking system that supports:
//! - Multiple graph views (Global, Local, Transitive DAG, Canonical)
//! - Group abstractions with dynamic resolution at query time
//! - Trust model based on reachability from root

pub mod events;
pub mod mock_substream;
