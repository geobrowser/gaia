//! Error types for the delivery-worker.

use thiserror::Error;

/// Errors that can occur during delivery.
#[derive(Debug, Error)]
pub enum DeliveryError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("webhook returned {status}: {body}")]
    WebhookError { status: u16, body: String },

    #[error("hmac error: {0}")]
    Hmac(String),
}

/// Errors that can occur during storage operations.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// Top-level worker error.
#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("delivery error: {0}")]
    Delivery(#[from] DeliveryError),

    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("telemetry error: {0}")]
    Telemetry(#[from] hermes_instrumentation::Error),

    #[error("configuration error: {0}")]
    Config(String),
}
