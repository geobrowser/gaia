//! Vote indexer library.
//!
//! This crate provides functionality for consuming vote events from Kafka
//! and indexing them into PostgreSQL.

pub mod consumer;
pub mod error;
pub mod handlers;
pub mod models;
pub mod storage;
