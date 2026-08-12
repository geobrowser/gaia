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
pub mod persistence;
pub mod stall;

#[cfg(test)]
pub(crate) mod test_utils;
