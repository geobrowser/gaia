//! Notification indexer library.
//!
//! Consumes governance events from Kafka and writes notifications
//! to the outbox for delivery to registered webhooks.

pub mod consumer;
pub mod error;
pub mod models;
pub mod storage;
