use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use rdkafka::Offset;
use rdkafka::TopicPartitionList;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{BaseConsumer, Consumer};
use tracing::debug;

/// Wait until the consumer group has committed past all the produced offsets.
///
/// `target_offsets` maps (topic, partition) -> last produced offset.
/// We wait until the committed offset for each partition exceeds the target
/// (committed offset is "next to read", so committed > target means done).
///
/// Returns the time spent waiting.
pub async fn wait_for_committed_past(
    broker: &str,
    group_id: &str,
    target_offsets: &HashMap<(String, i32), i64>,
    timeout_secs: u64,
) -> Result<Duration> {
    if target_offsets.is_empty() {
        return Ok(Duration::ZERO);
    }

    println!(
        "  Waiting for consumer group '{}' to commit past produced offsets:",
        group_id
    );
    for ((topic, partition), offset) in target_offsets {
        println!("    {}[{}] target offset: {}", topic, partition, offset);
    }

    // Single consumer for the entire polling loop. committed_offsets() always
    // makes a fresh broker round-trip so there's no stale-data concern.
    let consumer: BaseConsumer = ClientConfig::new()
        .set("bootstrap.servers", broker)
        .set("group.id", group_id)
        .create()
        .context("Failed to create consumer for lag check")?;

    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);

    loop {
        let mut all_committed = true;
        let mut remaining = 0i64;

        for ((topic, partition), target) in target_offsets {
            let mut tpl = TopicPartitionList::new();
            tpl.add_partition_offset(topic, *partition, Offset::Invalid)?;

            let committed = consumer
                .committed_offsets(tpl, Duration::from_secs(5))
                .with_context(|| {
                    format!(
                        "Failed to fetch committed offsets for group {} on {}[{}]",
                        group_id, topic, partition
                    )
                })?;

            let committed_offset = committed
                .find_partition(topic, *partition)
                .map(|e| match e.offset() {
                    Offset::Offset(o) => o,
                    _ => -1,
                })
                .unwrap_or(-1);

            debug!(
                topic,
                partition,
                committed_offset,
                target_offset = target,
                "Checking committed offset"
            );

            // Committed offset is "next to read", so committed > target means
            // the message at `target` has been consumed and committed.
            if committed_offset <= *target {
                all_committed = false;
                remaining += target - committed_offset + 1;
            }
        }

        if all_committed {
            println!("  All late scores committed");
            return Ok(start.elapsed());
        }

        if start.elapsed() > timeout {
            return Err(anyhow!(
                "Timeout after {}s waiting for consumer group '{}' to commit past produced offsets (remaining: ~{} messages)",
                timeout_secs,
                group_id,
                remaining
            ));
        }

        println!(
            "  Waiting for ~{} messages to be committed ({:.0}s elapsed)...",
            remaining,
            start.elapsed().as_secs_f64()
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
