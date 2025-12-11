//! Shared Kafka utilities for Hermes transformers.
//!
//! This crate provides common Kafka producer configuration and utilities
//! used by all Hermes transformer binaries.
//!
//! ## Usage
//!
//! ```ignore
//! use hermes_kafka::{create_producer, ProducerConfig};
//!
//! // Using environment variables (default)
//! let producer = create_producer("localhost:9092")?;
//!
//! // Or with explicit configuration
//! let config = ProducerConfig {
//!     broker: "localhost:9092".to_string(),
//!     client_id: "my-transformer".to_string(),
//!     username: None,
//!     password: None,
//!     ssl_ca_pem: None,
//! };
//! let producer = create_producer_with_config(&config)?;
//! ```

use std::env;

use anyhow::Result;
use rdkafka::config::ClientConfig;

/// Configuration for creating a Kafka producer.
#[derive(Debug, Clone)]
pub struct ProducerConfig {
    /// Kafka broker address (e.g., "localhost:9092")
    pub broker: String,
    /// Client ID for this producer
    pub client_id: String,
    /// SASL username (enables SASL/SSL if set)
    pub username: Option<String>,
    /// SASL password (required if username is set)
    pub password: Option<String>,
    /// Custom CA certificate in PEM format
    pub ssl_ca_pem: Option<String>,
}

impl ProducerConfig {
    /// Create a new ProducerConfig with the given broker and client_id.
    pub fn new(broker: impl Into<String>, client_id: impl Into<String>) -> Self {
        Self {
            broker: broker.into(),
            client_id: client_id.into(),
            username: None,
            password: None,
            ssl_ca_pem: None,
        }
    }

    /// Create a ProducerConfig from environment variables.
    ///
    /// # Environment Variables
    ///
    /// - `KAFKA_BROKER` - Broker address (uses provided default if not set)
    /// - `KAFKA_USERNAME` - SASL username (optional)
    /// - `KAFKA_PASSWORD` - SASL password (optional)
    /// - `KAFKA_SSL_CA_PEM` - Custom CA cert in PEM format (optional)
    pub fn from_env(default_broker: &str, client_id: impl Into<String>) -> Self {
        Self {
            broker: env::var("KAFKA_BROKER").unwrap_or_else(|_| default_broker.to_string()),
            client_id: client_id.into(),
            username: env::var("KAFKA_USERNAME").ok(),
            password: env::var("KAFKA_PASSWORD").ok(),
            ssl_ca_pem: env::var("KAFKA_SSL_CA_PEM").ok(),
        }
    }

    /// Set SASL credentials.
    pub fn with_credentials(mut self, username: String, password: String) -> Self {
        self.username = Some(username);
        self.password = Some(password);
        self
    }

    /// Set custom CA certificate.
    pub fn with_ssl_ca(mut self, ca_pem: String) -> Self {
        self.ssl_ca_pem = Some(ca_pem);
        self
    }
}

/// Create a Kafka producer with the given configuration.
///
/// Configures the producer with:
/// - zstd compression
/// - Optimized buffering settings for high throughput
/// - SASL/SSL authentication if credentials are provided
pub fn create_producer_with_config(config: &ProducerConfig) -> Result<rdkafka::producer::BaseProducer> {
    let mut client_config = ClientConfig::new();

    client_config
        .set("bootstrap.servers", &config.broker)
        .set("client.id", &config.client_id)
        .set("compression.type", "zstd")
        .set("message.timeout.ms", "5000")
        .set("queue.buffering.max.messages", "100000")
        .set("queue.buffering.max.kbytes", "1048576")
        .set("batch.num.messages", "10000");

    // If SASL credentials are provided, enable SASL/SSL (for managed Kafka)
    // Otherwise, use plaintext (for local development)
    if let (Some(username), Some(password)) = (&config.username, &config.password) {
        client_config
            .set("security.protocol", "SASL_SSL")
            .set("sasl.mechanisms", "PLAIN")
            .set("sasl.username", username)
            .set("sasl.password", password);

        // Use custom CA certificate if provided
        if let Some(ca_pem) = &config.ssl_ca_pem {
            client_config.set("ssl.ca.pem", ca_pem);
        }
    }

    Ok(client_config.create()?)
}

/// Create a Kafka producer using environment variables for configuration.
///
/// This is a convenience function that reads credentials from environment
/// variables and creates a producer.
///
/// # Environment Variables
///
/// - `KAFKA_USERNAME` - SASL username (optional, enables SASL/SSL if set)
/// - `KAFKA_PASSWORD` - SASL password (required if username is set)
/// - `KAFKA_SSL_CA_PEM` - Custom CA certificate in PEM format (optional)
///
/// # Arguments
///
/// * `broker` - Kafka broker address
/// * `client_id` - Client ID for this producer
pub fn create_producer(broker: &str, client_id: &str) -> Result<rdkafka::producer::BaseProducer> {
    let config = ProducerConfig {
        broker: broker.to_string(),
        client_id: client_id.to_string(),
        username: env::var("KAFKA_USERNAME").ok(),
        password: env::var("KAFKA_PASSWORD").ok(),
        ssl_ca_pem: env::var("KAFKA_SSL_CA_PEM").ok(),
    };

    create_producer_with_config(&config)
}

// Re-export commonly used rdkafka types for convenience
pub use rdkafka::message::{Header, OwnedHeaders};
pub use rdkafka::producer::{BaseProducer, BaseRecord, Producer};
