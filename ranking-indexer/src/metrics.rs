//! Prometheus metrics for the ranking-indexer consumer.
//!
//! The recorder and the `/metrics` listener come from `hermes_instrumentation::metrics`,
//! installed in `main`. The CronJob binaries that share this crate never install one, so
//! the calls here are no-ops in them. Every series is registered at zero by [`register`]
//! so it exists from startup: a counter that first appears at 1 has no earlier sample, so
//! `increase()` cannot see its first event.
//!
//! No failure counter covers the transient path that gives up. After
//! `MAX_TRANSIENT_ATTEMPTS` the process exits without committing, by design, so Kafka
//! redelivers on restart. Its counter would die with the process before a scrape could
//! read it; the container restart count is the signal for that path.
//!
//! There is deliberately no position gauge. Edits and membership events only arrive when
//! someone writes, so any "last processed" value would stall through quiet stretches.
//! Per-partition lag is already watched by kafka-exporter.

pub const MESSAGES_PROCESSED: &str = "ranking_indexer_messages_processed_total";
pub const MESSAGES_SKIPPED: &str = "ranking_indexer_messages_skipped_total";
pub const TRANSIENT_RETRIES: &str = "ranking_indexer_transient_retries_total";

/// Which subscribed topic a message came from, as a label value.
#[derive(Debug, Clone, Copy)]
pub enum Topic {
    Edits,
    Membership,
}

impl Topic {
    fn as_str(self) -> &'static str {
        match self {
            Topic::Edits => "edits",
            Topic::Membership => "membership",
        }
    }
}

/// Describe every counter and register it at zero.
pub fn register() {
    metrics::describe_counter!(
        MESSAGES_PROCESSED,
        "Messages applied to the ranks schema and committed, including membership event \
         types the indexer deliberately ignores (SPACE_LEFT)"
    );
    metrics::describe_counter!(
        MESSAGES_SKIPPED,
        "Messages committed past without being applied because they can never succeed \
         (malformed payload, unknown role or event type)"
    );
    metrics::describe_counter!(
        TRANSIENT_RETRIES,
        "Retries after a transient database or Kafka error; exhausting them exits the process"
    );

    for topic in [Topic::Edits, Topic::Membership] {
        metrics::counter!(MESSAGES_PROCESSED, "topic" => topic.as_str()).absolute(0);
        metrics::counter!(MESSAGES_SKIPPED, "topic" => topic.as_str()).absolute(0);
        metrics::counter!(TRANSIENT_RETRIES, "topic" => topic.as_str()).absolute(0);
    }
}

pub fn message_processed(topic: Topic) {
    metrics::counter!(MESSAGES_PROCESSED, "topic" => topic.as_str()).increment(1);
}

pub fn message_skipped(topic: Topic) {
    metrics::counter!(MESSAGES_SKIPPED, "topic" => topic.as_str()).increment(1);
}

pub fn transient_retry(topic: Topic) {
    metrics::counter!(TRANSIENT_RETRIES, "topic" => topic.as_str()).increment(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_exporter_prometheus::PrometheusBuilder;

    fn render_with(f: impl FnOnce()) -> String {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, f);
        handle.render()
    }

    #[test]
    fn register_exposes_every_counter_at_zero() {
        let rendered = render_with(register);
        for name in [MESSAGES_PROCESSED, MESSAGES_SKIPPED, TRANSIENT_RETRIES] {
            for topic in ["edits", "membership"] {
                let line = format!("{name}{{topic=\"{topic}\"}} 0");
                assert!(rendered.contains(&line), "missing `{line}` in:\n{rendered}");
            }
        }
    }

    #[test]
    fn counters_split_by_topic() {
        let rendered = render_with(|| {
            register();
            message_processed(Topic::Edits);
            message_processed(Topic::Edits);
            message_processed(Topic::Membership);
            message_skipped(Topic::Membership);
            transient_retry(Topic::Edits);
        });
        for line in [
            "ranking_indexer_messages_processed_total{topic=\"edits\"} 2",
            "ranking_indexer_messages_processed_total{topic=\"membership\"} 1",
            "ranking_indexer_messages_skipped_total{topic=\"membership\"} 1",
            "ranking_indexer_messages_skipped_total{topic=\"edits\"} 0",
            "ranking_indexer_transient_retries_total{topic=\"edits\"} 1",
        ] {
            assert!(rendered.contains(line), "missing `{line}` in:\n{rendered}");
        }
    }
}
