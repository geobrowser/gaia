//! Vote indexer library.
//!
//! This crate provides functionality for consuming vote events from Kafka
//! and indexing them into PostgreSQL.

pub mod consumer;
pub mod derived_scores;
pub mod error;
pub mod handlers;
pub mod metrics;
pub mod models;
pub mod new_entity_sweep;
pub mod storage;
pub mod votes_count_mirror;
pub mod write_retry;
