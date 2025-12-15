//! Kafka producer for Atlas
//!
//! Provides a wrapper around rdkafka's BaseProducer configured for
//! emitting canonical graph updates with ZSTD compression.

use rdkafka::config::ClientConfig;
use rdkafka::error::KafkaError;
use rdkafka::producer::{BaseProducer, BaseRecord, Producer};
use std::time::Duration;

/// Error types for producer operations
#[derive(Debug)]
pub enum ProducerError {
    /// Failed to create the Kafka producer
    Creation(KafkaError),
    /// Failed to send a message
    Send(KafkaError),
    /// Failed to flush messages
    Flush(KafkaError),
}

impl std::fmt::Display for ProducerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProducerError::Creation(e) => write!(f, "failed to create producer: {}", e),
            ProducerError::Send(e) => write!(f, "failed to send message: {}", e),
            ProducerError::Flush(e) => write!(f, "failed to flush messages: {}", e),
        }
    }
}

impl std::error::Error for ProducerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProducerError::Creation(e) => Some(e),
            ProducerError::Send(e) => Some(e),
            ProducerError::Flush(e) => Some(e),
        }
    }
}

/// Kafka producer for emitting canonical graph updates
///
/// Configured with ZSTD compression and sensible defaults for
/// high-throughput message production.
pub struct AtlasProducer {
    producer: BaseProducer,
    topic: String,
}

impl AtlasProducer {
    /// Create a new producer connected to the given broker
    ///
    /// # Arguments
    ///
    /// * `broker` - Kafka bootstrap server address (e.g., "localhost:9092")
    /// * `topic` - Topic to produce messages to (e.g., "topology.canonical")
    ///
    /// # Example
    ///
    /// ```ignore
    /// let producer = AtlasProducer::new("localhost:9092", "topology.canonical")?;
    /// ```
    pub fn new(broker: &str, topic: &str) -> Result<Self, ProducerError> {
        let mut config = ClientConfig::new();

        config
            .set("bootstrap.servers", broker)
            .set("client.id", "atlas-producer")
            .set("compression.type", "zstd")
            .set("message.timeout.ms", "5000")
            .set("queue.buffering.max.messages", "100000")
            .set("queue.buffering.max.kbytes", "1048576") // 1GB buffer
            .set("batch.num.messages", "10000");

        // If SASL credentials are provided, enable SASL/SSL (for managed Kafka)
        // Otherwise, use plaintext (for local development)
        if let (Ok(username), Ok(password)) = (
            std::env::var("KAFKA_USERNAME"),
            std::env::var("KAFKA_PASSWORD"),
        ) {
            config
                .set("security.protocol", "SASL_SSL")
                .set("sasl.mechanisms", "PLAIN")
                .set("sasl.username", &username)
                .set("sasl.password", &password);

            // Use custom CA certificate if provided (PEM format string)
            if let Ok(ca_pem) = std::env::var("KAFKA_SSL_CA_PEM") {
                config.set("ssl.ca.pem", &ca_pem);
            }
        }

        let producer = config.create().map_err(ProducerError::Creation)?;

        Ok(Self {
            producer,
            topic: topic.to_string(),
        })
    }

    /// Send a message to Kafka
    ///
    /// # Arguments
    ///
    /// * `key` - Message key (used for partitioning)
    /// * `payload` - Serialized message payload
    ///
    /// Note: This method does not automatically flush. Call `flush()` to ensure
    /// messages are delivered, or use `send_and_flush()` for immediate delivery.
    pub fn send(&self, key: &[u8], payload: &[u8]) -> Result<(), ProducerError> {
        let record = BaseRecord::to(&self.topic).key(key).payload(payload);

        self.producer
            .send(record)
            .map_err(|(e, _)| ProducerError::Send(e))?;

        Ok(())
    }

    /// Flush all buffered messages to Kafka
    ///
    /// Blocks until all messages are delivered or the timeout is reached.
    pub fn flush(&self) -> Result<(), ProducerError> {
        self.producer
            .flush(Duration::from_secs(5))
            .map_err(ProducerError::Flush)
    }

    /// Send a message and immediately flush
    ///
    /// Convenience method that combines `send()` and `flush()` for
    /// immediate delivery confirmation.
    pub fn send_and_flush(&self, key: &[u8], payload: &[u8]) -> Result<(), ProducerError> {
        self.send(key, payload)?;
        self.flush()
    }

    /// Get the topic this producer sends to
    pub fn topic(&self) -> &str {
        &self.topic
    }
}

impl std::fmt::Debug for AtlasProducer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AtlasProducer")
            .field("topic", &self.topic)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_producer_error_display() {
        // Just verify error types implement Display correctly
        let err = ProducerError::Send(KafkaError::NoMessageReceived);
        assert!(err.to_string().contains("failed to send message"));
    }
}
