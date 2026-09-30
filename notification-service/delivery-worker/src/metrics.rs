//! Prometheus metrics for delivery-worker.
//!
//! The recorder and the `/metrics` listener come from `hermes_instrumentation::metrics`,
//! installed in `main`. Every counter is registered at zero by [`register`] so its series
//! exists from startup: a counter that first appears at 1 has no earlier sample, so
//! `increase()` cannot see its first event, and for `outcome="failed"` that first event is
//! the one the alert exists to catch.
//!
//! delivery-worker reads Postgres, not Kafka, so kafka-exporter says nothing about it.
//! The backlog gauges are its position signal instead, and unlike a "last processed"
//! value they stay correct when nothing is happening: an idle outbox reads 0.

pub const DELIVERIES: &str = "delivery_worker_deliveries_total";
pub const CLAIM_ERRORS: &str = "delivery_worker_claim_errors_total";
pub const STALE_CLAIMS_RESET: &str = "delivery_worker_stale_claims_reset_total";
pub const PENDING: &str = "delivery_worker_pending_deliveries";
pub const IN_PROGRESS: &str = "delivery_worker_in_progress_deliveries";

/// How one delivery attempt ended.
#[derive(Debug, Clone, Copy)]
pub enum Outcome {
    /// The webhook accepted it (2xx, or 409 for a duplicate).
    Delivered,
    /// Rejected or unreachable, and rescheduled with backoff.
    Retried,
    /// Given up on for good: out of attempts, or the payload could not be serialized.
    /// The notification never reaches that webhook.
    Failed,
    /// The attempt ran but its result could not be written back, or the task panicked.
    /// The row stays `in_progress` until the stale-claim reaper resets it.
    Error,
}

impl Outcome {
    const ALL: [Outcome; 4] = [
        Outcome::Delivered,
        Outcome::Retried,
        Outcome::Failed,
        Outcome::Error,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Outcome::Delivered => "delivered",
            Outcome::Retried => "retried",
            Outcome::Failed => "failed",
            Outcome::Error => "error",
        }
    }
}

/// Describe every series and register the counters at zero.
///
/// The gauges are left unset until the first heartbeat reads them, so a failed backlog
/// query shows as a missing series rather than as a misleading 0.
pub fn register() {
    metrics::describe_counter!(
        DELIVERIES,
        "Webhook delivery attempts by outcome: delivered, retried (rescheduled with \
         backoff), failed (given up on for good) or error (result not recorded)"
    );
    metrics::describe_counter!(
        CLAIM_ERRORS,
        "Poll cycles whose claim of pending deliveries failed"
    );
    metrics::describe_counter!(
        STALE_CLAIMS_RESET,
        "in_progress deliveries reset to pending by the reaper after a worker died mid-delivery"
    );
    metrics::describe_gauge!(
        PENDING,
        "Deliveries in status pending, including retries still waiting out their backoff"
    );
    metrics::describe_gauge!(
        IN_PROGRESS,
        "Deliveries claimed by a worker and not yet resolved"
    );

    for outcome in Outcome::ALL {
        metrics::counter!(DELIVERIES, "outcome" => outcome.as_str()).absolute(0);
    }
    metrics::counter!(CLAIM_ERRORS).absolute(0);
    metrics::counter!(STALE_CLAIMS_RESET).absolute(0);
}

pub fn delivery(outcome: Outcome) {
    metrics::counter!(DELIVERIES, "outcome" => outcome.as_str()).increment(1);
}

pub fn claim_error() {
    metrics::counter!(CLAIM_ERRORS).increment(1);
}

pub fn stale_claims_reset(count: u64) {
    metrics::counter!(STALE_CLAIMS_RESET).increment(count);
}

pub fn set_pending(count: i64) {
    metrics::gauge!(PENDING).set(count as f64);
}

pub fn set_in_progress(count: i64) {
    metrics::gauge!(IN_PROGRESS).set(count as f64);
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
            "delivery_worker_deliveries_total{outcome=\"delivered\"} 0",
            "delivery_worker_deliveries_total{outcome=\"retried\"} 0",
            "delivery_worker_deliveries_total{outcome=\"failed\"} 0",
            "delivery_worker_deliveries_total{outcome=\"error\"} 0",
            "delivery_worker_claim_errors_total 0",
            "delivery_worker_stale_claims_reset_total 0",
        ] {
            assert!(rendered.contains(line), "missing `{line}` in:\n{rendered}");
        }
        // Unset until the first heartbeat query succeeds.
        assert!(
            !rendered.contains("delivery_worker_pending_deliveries "),
            "got:\n{rendered}"
        );
    }

    #[test]
    fn counters_and_gauges() {
        let rendered = render_with(|| {
            register();
            delivery(Outcome::Delivered);
            delivery(Outcome::Delivered);
            delivery(Outcome::Failed);
            claim_error();
            stale_claims_reset(3);
            set_pending(12);
            set_pending(0);
            set_in_progress(4);
        });
        for line in [
            "delivery_worker_deliveries_total{outcome=\"delivered\"} 2",
            "delivery_worker_deliveries_total{outcome=\"failed\"} 1",
            "delivery_worker_claim_errors_total 1",
            "delivery_worker_stale_claims_reset_total 3",
            "delivery_worker_pending_deliveries 0",
            "delivery_worker_in_progress_deliveries 4",
        ] {
            assert!(rendered.contains(line), "missing `{line}` in:\n{rendered}");
        }
    }
}
