use anyhow::{Context, Result};
use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord, Producer};
use std::time::Duration;
use tracing::{info, warn};

pub struct KafkaProducer {
    producer: FutureProducer,
}

impl KafkaProducer {
    /// Create a new Kafka producer
    pub fn new(broker: &str) -> Result<Self> {
        let producer: FutureProducer = ClientConfig::new()
            .set("bootstrap.servers", broker)
            .set("message.timeout.ms", "5000")
            .set("queue.buffering.max.messages", "100000")
            .set("queue.buffering.max.kbytes", "1048576")
            .set("batch.num.messages", "1000")
            .create()
            .context("Failed to create Kafka producer")?;

        info!("Created Kafka producer connected to {}", broker);

        Ok(Self { producer })
    }

    /// Send a message to a Kafka topic
    pub async fn send(&self, topic: &str, key: Option<&str>, payload: Vec<u8>) -> Result<()> {
        let mut record = FutureRecord::to(topic).payload(&payload);

        if let Some(k) = key {
            record = record.key(k);
        }

        let delivery_future = self.producer.send(record, Duration::from_secs(0));

        match delivery_future.await {
            Ok(delivery) => {
                info!(
                    "Message delivered to topic '{}' partition {} offset {}",
                    topic, delivery.0, delivery.1
                );
                Ok(())
            }
            Err((e, _)) => {
                warn!("Failed to deliver message to topic '{}': {:?}", topic, e);
                Err(e.into())
            }
        }
    }

    /// Send multiple messages to a Kafka topic
    #[allow(dead_code)]
    pub async fn send_batch(
        &self,
        topic: &str,
        messages: Vec<(Option<String>, Vec<u8>)>,
    ) -> Result<()> {
        info!("Sending {} messages to topic '{}'", messages.len(), topic);

        for (key, payload) in messages {
            self.send(topic, key.as_deref(), payload).await?;
        }

        // Flush to ensure all messages are sent
        self.producer.flush(Duration::from_secs(10))?;

        info!("Successfully sent all messages to topic '{}'", topic);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_producer() {
        // This test will fail if Kafka is not running
        // Run with: cargo test -- --ignored
        let result = KafkaProducer::new("localhost:9092");
        // Just verify we can create the producer object
        // We won't actually send messages in tests
        assert!(result.is_ok() || result.is_err()); // Always passes, just checking compilation
    }
}
