//! # Actions Indexer Pipeline
//! This crate defines the core traits and modules for processing actions within
//! the indexer.
//! It includes modules for consuming, loading, processing, and orchestrating
//! actions, along with error handling.
pub mod consumer;
pub mod loader;
pub mod orchestrator;
pub mod processor;

pub mod errors;
