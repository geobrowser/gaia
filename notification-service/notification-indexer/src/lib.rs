//! Notification indexer library.
//!
//! Consumes governance events from Kafka and writes notifications
//! to the outbox for delivery to registered webhooks.

pub mod consumer;
pub mod consumer_lag;
pub mod error;
pub mod health;
pub mod ids;
pub mod models;
pub mod storage;
