//! Delivery worker library.
//!
//! Polls the notification outbox for pending deliveries and POSTs
//! to registered webhooks with HMAC-SHA256 signatures.

pub mod deliver;
pub mod error;
pub mod health;
pub mod storage;
