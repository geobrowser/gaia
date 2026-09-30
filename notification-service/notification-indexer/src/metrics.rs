//! Prometheus metrics for notification-indexer.
//!
//! The recorder and the `/metrics` listener come from `hermes_instrumentation::metrics`,
//! installed in `main`. Every counter is registered at zero by [`register`] so its series
//! exists from startup: a counter that first appears at 1 has no earlier sample, so
//! `increase()` cannot see its first event, and for `reason="db_error"` that first event is
//! the one the alert exists to catch.
//!
//! A `db_error` is quieter than the log line makes it sound. The message's offset is left
//! uncommitted "so it is retried on restart", but the next message on the same partition
//! that succeeds commits past it, and nothing seeks back. Unless the pod restarts first,
//! that message's notifications are never written.
//!
//! There is deliberately no position gauge. Both topics only carry messages when someone
//! acts, so any "last processed" value would stall through quiet stretches. Per-partition
//! lag is already watched by kafka-exporter, which is why the in-process `LagMonitor`
//! value stays a log field and is not exported here too.

pub const EVENTS_PROCESSED: &str = "notification_indexer_events_processed_total";
pub const EVENTS_FAILED: &str = "notification_indexer_events_failed_total";
pub const NOTIFICATIONS_INSERTED: &str = "notification_indexer_notifications_inserted_total";
pub const POLLER_ERRORS: &str = "notification_indexer_poller_errors_total";

/// Which Kafka consumer handled a message.
#[derive(Debug, Clone, Copy)]
pub enum Consumer {
    Governance,
    KnowledgeEdits,
}

impl Consumer {
    const ALL: [Consumer; 2] = [Consumer::Governance, Consumer::KnowledgeEdits];

    fn as_str(self) -> &'static str {
        match self {
            Consumer::Governance => "governance",
            Consumer::KnowledgeEdits => "knowledge_edits",
        }
    }
}

/// Why an event produced no notifications.
#[derive(Debug, Clone, Copy)]
pub enum FailReason {
    /// Malformed payload or ids; committed past on purpose, since a retry cannot succeed.
    Unprocessable,
    /// A database call failed; left uncommitted, but see the module doc.
    DbError,
}

impl FailReason {
    const ALL: [FailReason; 2] = [FailReason::Unprocessable, FailReason::DbError];

    fn as_str(self) -> &'static str {
        match self {
            FailReason::Unprocessable => "unprocessable",
            FailReason::DbError => "db_error",
        }
    }
}

/// The background pollers, which fail independently of either consumer.
#[derive(Debug, Clone, Copy)]
pub enum Poller {
    /// Expired-proposal rejection notifications.
    Rejection,
    /// Entity vote-threshold notifications.
    VoteThreshold,
    /// Outbox and delivery retention cleanup.
    Retention,
}

impl Poller {
    const ALL: [Poller; 3] = [Poller::Rejection, Poller::VoteThreshold, Poller::Retention];

    fn as_str(self) -> &'static str {
        match self {
            Poller::Rejection => "rejection",
            Poller::VoteThreshold => "vote_threshold",
            Poller::Retention => "retention",
        }
    }
}

/// Describe every counter and register it at zero.
pub fn register() {
    metrics::describe_counter!(
        EVENTS_PROCESSED,
        "Relevant events (governance events; bounty and comment edits) whose notifications \
         were written, or that had no recipients"
    );
    metrics::describe_counter!(
        EVENTS_FAILED,
        "Relevant events that produced no notifications. reason=\"db_error\" is left \
         uncommitted, but a later commit on the same partition supersedes it, so it is \
         only retried if the pod restarts first"
    );
    metrics::describe_counter!(
        NOTIFICATIONS_INSERTED,
        "New notification_outbox rows (one per recipient), from every source; duplicates \
         skipped by the idempotency key are not counted"
    );
    metrics::describe_counter!(
        POLLER_ERRORS,
        "Database errors in the background pollers; each one skips the rest of that tick"
    );

    for consumer in Consumer::ALL {
        metrics::counter!(EVENTS_PROCESSED, "consumer" => consumer.as_str()).absolute(0);
        for reason in FailReason::ALL {
            metrics::counter!(
                EVENTS_FAILED,
                "consumer" => consumer.as_str(),
                "reason" => reason.as_str()
            )
            .absolute(0);
        }
    }
    metrics::counter!(NOTIFICATIONS_INSERTED).absolute(0);
    for poller in Poller::ALL {
        metrics::counter!(POLLER_ERRORS, "poller" => poller.as_str()).absolute(0);
    }
}

pub fn event_processed(consumer: Consumer) {
    metrics::counter!(EVENTS_PROCESSED, "consumer" => consumer.as_str()).increment(1);
}

pub fn event_failed(consumer: Consumer, reason: FailReason) {
    metrics::counter!(
        EVENTS_FAILED,
        "consumer" => consumer.as_str(),
        "reason" => reason.as_str()
    )
    .increment(1);
}

pub fn notifications_inserted(count: u64) {
    metrics::counter!(NOTIFICATIONS_INSERTED).increment(count);
}

pub fn poller_error(poller: Poller) {
    metrics::counter!(POLLER_ERRORS, "poller" => poller.as_str()).increment(1);
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
        for line in [
            "notification_indexer_events_processed_total{consumer=\"governance\"} 0",
            "notification_indexer_events_processed_total{consumer=\"knowledge_edits\"} 0",
            "notification_indexer_events_failed_total{consumer=\"governance\",reason=\"db_error\"} 0",
            "notification_indexer_events_failed_total{consumer=\"knowledge_edits\",reason=\"unprocessable\"} 0",
            "notification_indexer_notifications_inserted_total 0",
            "notification_indexer_poller_errors_total{poller=\"rejection\"} 0",
            "notification_indexer_poller_errors_total{poller=\"vote_threshold\"} 0",
            "notification_indexer_poller_errors_total{poller=\"retention\"} 0",
        ] {
            assert!(rendered.contains(line), "missing `{line}` in:\n{rendered}");
        }
    }

    #[test]
    fn counters_keep_their_labels_apart() {
        let rendered = render_with(|| {
            register();
            event_processed(Consumer::Governance);
            event_processed(Consumer::Governance);
            event_failed(Consumer::KnowledgeEdits, FailReason::DbError);
            notifications_inserted(5);
            notifications_inserted(0);
            poller_error(Poller::Retention);
        });
        for line in [
            "notification_indexer_events_processed_total{consumer=\"governance\"} 2",
            "notification_indexer_events_processed_total{consumer=\"knowledge_edits\"} 0",
            "notification_indexer_events_failed_total{consumer=\"knowledge_edits\",reason=\"db_error\"} 1",
            "notification_indexer_events_failed_total{consumer=\"governance\",reason=\"db_error\"} 0",
            "notification_indexer_notifications_inserted_total 5",
            "notification_indexer_poller_errors_total{poller=\"retention\"} 1",
        ] {
            assert!(rendered.contains(line), "missing `{line}` in:\n{rendered}");
        }
    }
}
