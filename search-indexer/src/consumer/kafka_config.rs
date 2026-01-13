//! Shared Kafka configuration utilities.
//!
//! Provides common configuration for Kafka consumers.

use rdkafka::config::ClientConfig;
use std::env;
use tracing::info;

/// Create a base Kafka client configuration with common settings.
///
/// Configures:
/// - Bootstrap servers
/// - Consumer group ID
/// - Manual commit (for at-least-once delivery)
/// - Auto offset reset to earliest
/// - Session timeout
/// - SASL/SSL authentication (if credentials provided via environment variables)
///
/// # Environment Variables
///
/// - `KAFKA_USERNAME`: SASL username (optional, enables SASL/SSL if provided with password)
/// - `KAFKA_PASSWORD`: SASL password (optional, enables SASL/SSL if provided with username)
/// - `KAFKA_SSL_CA_PEM`: Custom CA certificate PEM (optional)
pub fn create_client_config(brokers: &str, group_id: &str) -> ClientConfig {
    let mut client_config = ClientConfig::new();

    client_config
        .set("bootstrap.servers", brokers)
        .set("group.id", group_id)
        .set("enable.auto.commit", "false") // Manual commit for at-least-once delivery
        .set("auto.offset.reset", "earliest") // Start from beginning if no committed offset
        .set("session.timeout.ms", "6000");

    // Configure SASL/SSL if credentials are provided
    let username = env::var("KAFKA_USERNAME").ok();
    let password = env::var("KAFKA_PASSWORD").ok();
    let ssl_ca_pem = env::var("KAFKA_SSL_CA_PEM").ok();

    if let (Some(username), Some(password)) = (&username, &password) {
        client_config
            .set("security.protocol", "SASL_SSL")
            .set("sasl.mechanisms", "PLAIN")
            .set("sasl.username", username)
            .set("sasl.password", password);

        if let Some(ca_pem) = &ssl_ca_pem {
            client_config.set("ssl.ca.pem", ca_pem);
        }

        info!(group_id = %group_id, "Configured Kafka consumer with SSL authentication");
    } else {
        info!(group_id = %group_id, "Using plaintext Kafka connection");
    }

    client_config
}
