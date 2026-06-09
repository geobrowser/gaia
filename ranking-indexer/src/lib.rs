//! ranking-indexer: consumes `knowledge.edits`, maintains the private `ranks`
//! working schema, and (future) projects aggregated `RANK_POSITION` relations
//! back into the public graph.

pub mod consumer;
pub mod dedup;
pub mod detect;
pub mod eligibility;
pub mod error;
pub mod models;
pub mod recompute;
pub mod storage;
