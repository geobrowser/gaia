use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord, Producer};
use std::time::{Duration, Instant};
use tracing::warn;

/// A Kafka event ready to send.
pub struct KafkaEvent {
    pub topic: String,
    pub key: Option<String>,
    pub payload: Vec<u8>,
}

/// Summary stats from a send operation.
pub struct SendStats {
    pub total: usize,
    pub errors: usize,
    pub duration: Duration,
}

/// High-throughput Kafka sender optimized for load testing.
pub struct LoadTestSender {
    producer: FutureProducer,
}

impl LoadTestSender {
    pub fn new(broker: &str) -> Result<Self> {
        let producer: FutureProducer = ClientConfig::new()
            .set("bootstrap.servers", broker)
            .set("message.timeout.ms", "30000")
            .set("queue.buffering.max.messages", "500000")
            .set("queue.buffering.max.kbytes", "2097152")
            .set("batch.num.messages", "10000")
            .set("linger.ms", "5")
            .create()
            .context("Failed to create Kafka producer")?;

        Ok(Self { producer })
    }

    /// Send all events to Kafka with a progress bar.
    ///
    /// Sends in chunks to maintain backpressure while keeping throughput high.
    pub async fn send_all(&self, events: Vec<KafkaEvent>) -> Result<SendStats> {
        let total = events.len();
        let start = Instant::now();

        let pb = ProgressBar::new(total as u64);
        pb.set_style(
            ProgressStyle::with_template(
                "  Sending [{bar:40.cyan/blue}] {pos}/{len} ({per_sec}) ETA {eta}",
            )
            .unwrap()
            .progress_chars("=>-"),
        );

        let mut errors = 0usize;
        let chunk_size = 5_000;

        for chunk in events.chunks(chunk_size) {
            let mut futures = Vec::with_capacity(chunk.len());

            for event in chunk {
                let mut record = FutureRecord::to(&event.topic).payload(&event.payload);
                if let Some(ref k) = event.key {
                    record = record.key(k.as_str());
                }

                match self.producer.send_result(record) {
                    Ok(future) => futures.push(future),
                    Err((err, _)) => {
                        errors += 1;
                        if errors <= 10 {
                            warn!("Send enqueue error: {:?}", err);
                        }
                    }
                }
            }

            // Await all futures in this chunk
            for future in futures {
                match future.await {
                    Ok(Ok(_)) => {}
                    Ok(Err((err, _))) => {
                        errors += 1;
                        if errors <= 10 {
                            warn!("Delivery error: {:?}", err);
                        }
                    }
                    Err(_cancelled) => {
                        errors += 1;
                    }
                }
            }

            pb.inc(chunk.len() as u64);
        }

        // Final flush to ensure everything is delivered
        if let Err(e) = self.producer.flush(Duration::from_secs(30)) {
            warn!("Flush error: {:?}", e);
        }

        pb.finish_and_clear();

        Ok(SendStats {
            total,
            errors,
            duration: start.elapsed(),
        })
    }
}
