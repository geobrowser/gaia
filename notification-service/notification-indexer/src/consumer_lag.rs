//! Background consumer lag monitor.
//!
//! Runs on a dedicated OS thread with its own lightweight Kafka connection
//! so it never blocks the async runtime. Publishes the latest lag value to
//! a shared atomic that the heartbeat reads lock-free.

use rdkafka::{
    config::ClientConfig,
    consumer::{BaseConsumer, Consumer},
    TopicPartitionList,
};
use std::env;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tracing::debug;

use crate::error::IndexerError;

/// Background lag monitor that periodically queries Kafka for consumer group lag.
pub struct LagMonitor {
    lag: Arc<AtomicI64>,
    _handle: std::thread::JoinHandle<()>,
}

impl LagMonitor {
    /// Start a background lag monitor. Creates its own Kafka connection.
    /// Returns immediately; the monitor runs until the process exits.
    pub fn start(
        brokers: &str,
        group_id: &str,
        topic: String,
        poll_interval_secs: u64,
    ) -> Result<Self, IndexerError> {
        let lag = Arc::new(AtomicI64::new(-1));
        let lag_writer = lag.clone();

        // Build a lightweight BaseConsumer just for metadata queries.
        // No subscription needed — we only use admin-style APIs.
        let mut config = ClientConfig::new();
        config
            .set("bootstrap.servers", brokers)
            .set("group.id", group_id)
            .set("enable.auto.commit", "false")
            .set("session.timeout.ms", "10000");

        // Reuse SASL/SSL config from environment
        if let Ok(username) = env::var("KAFKA_USERNAME") {
            config.set("security.protocol", "SASL_SSL");
            config.set("sasl.mechanisms", "PLAIN");
            config.set("sasl.username", &username);
            if let Ok(password) = env::var("KAFKA_PASSWORD") {
                config.set("sasl.password", &password);
            }
        }
        if let Ok(ca_pem) = env::var("KAFKA_SSL_CA_PEM") {
            config.set("ssl.ca.pem", &ca_pem);
        }

        let base_consumer: BaseConsumer = config.create()?;
        let interval = std::time::Duration::from_secs(poll_interval_secs);
        let timeout = std::time::Duration::from_secs(2);

        let handle = std::thread::Builder::new()
            .name("lag-monitor".into())
            .spawn(move || loop {
                let lag_value = Self::query_lag(&base_consumer, &topic, timeout);
                lag_writer.store(lag_value, Ordering::Relaxed);
                std::thread::sleep(interval);
            })
            .expect("failed to spawn lag-monitor thread");

        Ok(Self {
            lag,
            _handle: handle,
        })
    }

    /// Read the latest lag value. Returns -1 if lag is unknown.
    pub fn get(&self) -> i64 {
        self.lag.load(Ordering::Relaxed)
    }

    /// Query total lag across all partitions of a topic for a consumer group.
    fn query_lag(consumer: &BaseConsumer, topic: &str, timeout: std::time::Duration) -> i64 {
        // Get topic metadata to discover partition count
        let metadata = match consumer.fetch_metadata(Some(topic), timeout) {
            Ok(m) => m,
            Err(e) => {
                debug!(error = %e, "Lag monitor: failed to fetch metadata");
                return -1;
            }
        };

        let topic_metadata = match metadata.topics().first() {
            Some(t) if !t.partitions().is_empty() => t,
            _ => return -1,
        };

        // Build TPL for all partitions
        let mut tpl = TopicPartitionList::new();
        for p in topic_metadata.partitions() {
            tpl.add_partition(topic, p.id());
        }

        // Get committed offsets for the consumer group
        let committed = match consumer.committed_offsets(tpl, timeout) {
            Ok(c) => c,
            Err(e) => {
                debug!(error = %e, "Lag monitor: failed to fetch committed offsets");
                return -1;
            }
        };

        let mut total_lag: i64 = 0;

        for elem in committed.elements() {
            let committed_offset = match elem.offset() {
                rdkafka::Offset::Offset(o) => o,
                _ => continue,
            };

            match consumer.fetch_watermarks(elem.topic(), elem.partition(), timeout) {
                Ok((_, high_watermark)) => {
                    total_lag += (high_watermark - committed_offset).max(0);
                }
                Err(e) => {
                    debug!(
                        error = %e,
                        partition = elem.partition(),
                        "Lag monitor: failed to fetch watermarks"
                    );
                    return -1;
                }
            }
        }

        total_lag
    }
}
