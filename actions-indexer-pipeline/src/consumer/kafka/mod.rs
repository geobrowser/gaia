//! Kafka consumer module for the actions indexer pipeline.
//!
//! Provides configuration and utilities for consuming from Kafka topics.

mod config;
mod conversion;
mod provider;

pub use config::ConsumerConfig;
pub use conversion::hermes_vote_to_action_raw;
pub use provider::KafkaStreamProvider;

