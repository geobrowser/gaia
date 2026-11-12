//! Delivery worker library.
//!
//! Polls the notification outbox for pending deliveries and POSTs
//! to registered webhooks with HMAC-SHA256 signatures.

pub mod deliver;
pub mod error;
pub mod geodb_claim;
pub mod health;
pub mod metrics;
pub mod shadow;
pub mod storage;
pub mod webhooks_file;
