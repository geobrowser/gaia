//! ranking-indexer: consumes `knowledge.edits` and `space.membership`,
//! maintains the private `ranks` working schema (including its own view of
//! space membership), and projects aggregated `RANK_POSITION` relations back
//! into the public graph.

pub mod consumer;
pub mod dedup;
pub mod detect;
pub mod eligibility;
pub mod error;
pub mod membership;
pub mod models;
pub mod publish;
pub mod recompute;
pub mod scoring;
pub mod storage;
