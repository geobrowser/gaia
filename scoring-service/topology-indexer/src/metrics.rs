//! Prometheus metrics for topology-indexer.
//!
//! The recorder and the `/metrics` listener come from `hermes_instrumentation::metrics`,
//! installed in `main`. Every counter is registered at zero by [`register`] so its series
//! exists from startup: a counter that first appears at 1 has no earlier sample, so
//! `increase()` cannot see its first event, and for [`DIFFS_DROPPED`] that first event is
//! the one the alert exists to catch.
//!
//! Since GEO-3101 a diff whose write fails transiently is retried and then halts the
//! process without committing, so [`DIFFS_DROPPED`] only counts diffs skipped for a
//! permanent, data-level error. [`WRITE_RETRIES`] counts the retries; [`HALTED`] is 1
//! for the few seconds between deciding to halt and exiting.
//!
//! There is deliberately no position gauge. topology-indexer consumes
//! `topology.canonical`, which only carries a message when the canonical graph changes, so
//! any "last processed" value would stall through quiet stretches. Per-partition lag is
//! already watched by kafka-exporter.

pub const DIFFS_APPLIED: &str = "topology_indexer_diffs_applied_total";
pub const DIFFS_DROPPED: &str = "topology_indexer_diffs_dropped_total";
pub const MESSAGES_UNPARSEABLE: &str = "topology_indexer_messages_unparseable_total";
pub const WRITE_RETRIES: &str = "topology_indexer_write_retries_total";
pub const HALTED: &str = "topology_indexer_halted";

/// Describe every counter and register it at zero.
pub fn register() {
    metrics::describe_counter!(
        DIFFS_APPLIED,
        "Canonical-graph diffs whose distance changes were committed to Postgres"
    );
    metrics::describe_counter!(
        DIFFS_DROPPED,
        "Diffs skipped because they failed to apply for a permanent, data-level reason \
         (SQLSTATE 22/23, or a distance out of range). A transient failure halts the \
         indexer instead of dropping, so this stays at 0 unless a diff can never apply"
    );
    metrics::describe_counter!(
        WRITE_RETRIES,
        "Diff writes retried after a transient database error"
    );
    metrics::describe_gauge!(
        HALTED,
        "1 while topology-indexer is halting on a diff that failed transiently on every \
         attempt; the process exits without committing so the restart re-reads it"
    );
    metrics::describe_counter!(
        MESSAGES_UNPARSEABLE,
        "Kafka messages that failed to decode and were committed past"
    );

    metrics::counter!(DIFFS_APPLIED).absolute(0);
    metrics::counter!(DIFFS_DROPPED).absolute(0);
    metrics::counter!(MESSAGES_UNPARSEABLE).absolute(0);
    metrics::counter!(WRITE_RETRIES).absolute(0);
    metrics::gauge!(HALTED).set(0.0);
}

pub fn diffs_applied(count: u64) {
    metrics::counter!(DIFFS_APPLIED).increment(count);
}

pub fn diffs_dropped(count: u64) {
    metrics::counter!(DIFFS_DROPPED).increment(count);
}

pub fn message_unparseable() {
    metrics::counter!(MESSAGES_UNPARSEABLE).increment(1);
}

pub fn write_retried() {
    metrics::counter!(WRITE_RETRIES).increment(1);
}

pub fn halted() {
    metrics::gauge!(HALTED).set(1.0);
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
            "topology_indexer_diffs_applied_total 0",
            "topology_indexer_diffs_dropped_total 0",
            "topology_indexer_messages_unparseable_total 0",
            "topology_indexer_write_retries_total 0",
            "topology_indexer_halted 0",
        ] {
            assert!(rendered.contains(line), "missing `{line}` in:\n{rendered}");
        }
    }

    #[test]
    fn counters_accumulate() {
        let rendered = render_with(|| {
            register();
            diffs_applied(3);
            diffs_applied(2);
            diffs_dropped(4);
            message_unparseable();
            write_retried();
            halted();
        });
        for line in [
            "topology_indexer_diffs_applied_total 5",
            "topology_indexer_diffs_dropped_total 4",
            "topology_indexer_messages_unparseable_total 1",
            "topology_indexer_write_retries_total 1",
            "topology_indexer_halted 1",
        ] {
            assert!(rendered.contains(line), "missing `{line}` in:\n{rendered}");
        }
    }
}
