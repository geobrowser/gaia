//! Prometheus metrics specific to kg-indexer.
//!
//! The processed-position gauges are the shared `hermes_latest_processed_block{,_timestamp}`
//! from `hermes_instrumentation::metrics`, so kg-indexer sits under the same
//! `HermesBehindChainTip` rule as atlas and hermes-pipeline. What it means for kg-indexer
//! to have processed a block is defined by `BlockBuffer::processed_position` in `main.rs`.
//!
//! Everything here is a counter. Each one is registered at zero by [`register`] so the
//! series exists from startup: a counter that first appears at 1 has no earlier sample,
//! so `increase()` cannot see its first event, and that first event is the one an alert
//! on dropped blocks exists to catch.

pub const BLOCKS_PROCESSED: &str = "kg_indexer_blocks_processed_total";
pub const EVENTS_PROCESSED: &str = "kg_indexer_events_processed_total";
pub const BATCH_RETRIES: &str = "kg_indexer_batch_retries_total";
pub const BLOCKS_DROPPED: &str = "kg_indexer_blocks_dropped_total";
pub const MESSAGES_UNPARSEABLE: &str = "kg_indexer_messages_unparseable_total";

/// Describe every counter and register it at zero.
pub fn register() {
    metrics::describe_counter!(
        BLOCKS_PROCESSED,
        "Blocks whose events were written to Postgres and committed"
    );
    metrics::describe_counter!(
        EVENTS_PROCESSED,
        "Events written to Postgres as part of a committed block"
    );
    metrics::describe_counter!(
        BATCH_RETRIES,
        "Block transactions retried after a transient failure"
    );
    metrics::describe_counter!(
        BLOCKS_DROPPED,
        "Blocks whose transaction failed on every attempt and whose events were skipped; \
         transient=\"true\" is the statement-timeout / connection class that GEO-2884 loses"
    );
    metrics::describe_counter!(
        MESSAGES_UNPARSEABLE,
        "Kafka messages that failed to decode and were committed past"
    );

    metrics::counter!(BLOCKS_PROCESSED).absolute(0);
    metrics::counter!(EVENTS_PROCESSED).absolute(0);
    metrics::counter!(BATCH_RETRIES).absolute(0);
    metrics::counter!(BLOCKS_DROPPED, "transient" => "true").absolute(0);
    metrics::counter!(BLOCKS_DROPPED, "transient" => "false").absolute(0);
    metrics::counter!(MESSAGES_UNPARSEABLE).absolute(0);
}

pub fn block_processed(event_count: u64) {
    metrics::counter!(BLOCKS_PROCESSED).increment(1);
    metrics::counter!(EVENTS_PROCESSED).increment(event_count);
}

pub fn batch_retried() {
    metrics::counter!(BATCH_RETRIES).increment(1);
}

/// A block whose transaction failed for good. Its offsets are superseded by the next
/// block's commit, so the events are gone until replayed by hand.
pub fn block_dropped(transient: bool) {
    let transient = if transient { "true" } else { "false" };
    metrics::counter!(BLOCKS_DROPPED, "transient" => transient).increment(1);
}

pub fn message_unparseable() {
    metrics::counter!(MESSAGES_UNPARSEABLE).increment(1);
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
            "kg_indexer_blocks_processed_total 0",
            "kg_indexer_events_processed_total 0",
            "kg_indexer_batch_retries_total 0",
            "kg_indexer_blocks_dropped_total{transient=\"true\"} 0",
            "kg_indexer_blocks_dropped_total{transient=\"false\"} 0",
            "kg_indexer_messages_unparseable_total 0",
        ] {
            assert!(rendered.contains(line), "missing `{line}` in:\n{rendered}");
        }
    }

    #[test]
    fn block_dropped_counts_by_cause() {
        let rendered = render_with(|| {
            register();
            block_dropped(true);
            block_dropped(true);
            block_dropped(false);
        });
        assert!(
            rendered.contains("kg_indexer_blocks_dropped_total{transient=\"true\"} 2"),
            "got:\n{rendered}"
        );
        assert!(
            rendered.contains("kg_indexer_blocks_dropped_total{transient=\"false\"} 1"),
            "got:\n{rendered}"
        );
    }

    #[test]
    fn block_processed_counts_blocks_and_events() {
        let rendered = render_with(|| {
            block_processed(3);
            block_processed(4);
        });
        assert!(
            rendered.contains("kg_indexer_blocks_processed_total 2"),
            "got:\n{rendered}"
        );
        assert!(
            rendered.contains("kg_indexer_events_processed_total 7"),
            "got:\n{rendered}"
        );
    }
}
