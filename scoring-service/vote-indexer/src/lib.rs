//! Vote indexer library.
//!
//! This crate provides functionality for consuming vote events from Kafka
//! and indexing them into PostgreSQL.

pub mod batch;
pub mod consumer;
pub mod derived_scores;
pub mod entity_scores_default;
#[cfg(test)]
mod entity_scores_fixture;
pub mod entity_scores_mirror;
pub mod error;
pub mod geodb_reads_shadow;
pub mod handlers;
pub mod metrics;
pub mod models;
pub mod new_entity_sweep;
pub mod ranking_snapshot;
pub mod scores_snapshot;
pub mod storage;
pub mod votes_count_mirror;
pub mod write_retry;

#[cfg(test)]
mod pg_writes_off_tests;
