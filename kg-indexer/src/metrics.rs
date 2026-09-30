//! Prometheus metrics specific to kg-indexer.
//!
//! The processed-position gauges are the shared `hermes_latest_processed_block{,_timestamp}`
//! from `hermes_instrumentation::metrics`, so kg-indexer sits under the same
//! `HermesBehindChainTip` rule as atlas and hermes-pipeline. What it means for kg-indexer
//! to have processed a block is defined by `BlockBuffer::processed_position` in `main.rs`.
//!
//! Since GEO-2884 a block is never dropped for a transient reason: the consumer
//! retries it and then halts (exits without committing) so the restart re-reads it.
//! `kg_indexer_blocks_dropped_total{transient="true"}` therefore stays at 0 and is
//! kept only so a regression shows up on the existing alert. `transient="false"` is
//! a block skipped for a permanent, data-level error. `kg_indexer_halted` is set to
//! 1 for the few seconds between deciding to halt and exiting, long enough to be
//! scraped, which is what `IndexerHalted` alerts on.
//!
//! Everything else here is a counter. Each one is registered at zero by [`register`] so the
//! series exists from startup: a counter that first appears at 1 has no earlier sample,
//! so `increase()` cannot see its first event, and that first event is the one an alert
//! on dropped blocks exists to catch.

pub const BLOCKS_PROCESSED: &str = "kg_indexer_blocks_processed_total";
pub const EVENTS_PROCESSED: &str = "kg_indexer_events_processed_total";
pub const BATCH_RETRIES: &str = "kg_indexer_batch_retries_total";
pub const BLOCKS_DROPPED: &str = "kg_indexer_blocks_dropped_total";
pub const MESSAGES_UNPARSEABLE: &str = "kg_indexer_messages_unparseable_total";
pub const HALTED: &str = "kg_indexer_halted";

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
        "Block transactions retried after a transient failure (kg-indexer's write-retry \
         counter; the other indexers call theirs *_write_retries_total)"
    );
    metrics::describe_counter!(
        BLOCKS_DROPPED,
        "Blocks skipped because their transaction failed. transient=\"false\" is a \
         permanent, data-level error (the block can never be written); transient=\"true\" \
         should stay at 0, because a transient failure halts the consumer instead (GEO-2884)"
    );
    metrics::describe_gauge!(
        HALTED,
        "1 while kg-indexer is halting on a block that failed transiently on every attempt; \
         the process exits without committing so the restart re-reads the block"
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
    metrics::gauge!(HALTED).set(0.0);
}

pub fn block_processed(event_count: u64) {
    metrics::counter!(BLOCKS_PROCESSED).increment(1);
    metrics::counter!(EVENTS_PROCESSED).increment(event_count);
}

pub fn batch_retried() {
    metrics::counter!(BATCH_RETRIES).increment(1);
}

/// A block skipped for a permanent error. Its offsets are superseded by the next
/// block's commit, so the events are gone until replayed by hand.
pub fn block_dropped(transient: bool) {
    let transient = if transient { "true" } else { "false" };
    metrics::counter!(BLOCKS_DROPPED, "transient" => transient).increment(1);
}

pub fn message_unparseable() {
    metrics::counter!(MESSAGES_UNPARSEABLE).increment(1);
}

/// The consumer is about to exit without committing the block it is stuck on.
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
            "kg_indexer_blocks_processed_total 0",
            "kg_indexer_events_processed_total 0",
            "kg_indexer_batch_retries_total 0",
            "kg_indexer_blocks_dropped_total{transient=\"true\"} 0",
            "kg_indexer_blocks_dropped_total{transient=\"false\"} 0",
            "kg_indexer_messages_unparseable_total 0",
            "kg_indexer_halted 0",
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
    fn halted_sets_the_gauge() {
        let rendered = render_with(|| {
            register();
            halted();
        });
        assert!(rendered.contains("kg_indexer_halted 1"), "got:\n{rendered}");
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
