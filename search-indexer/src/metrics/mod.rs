//! Metrics module for the search indexer orchestrator.
//!
//! Provides metrics tracking for events processed and documents indexed.
//!
//! The counters are plain atomics, updated on the hot path by the processor and
//! loader. [`SearchIndexerMetrics::publish`] copies them into the Prometheus
//! recorder that `hermes_instrumentation::metrics::install` sets up, so exposing
//! them adds nothing to that path. The orchestrator publishes once at startup and
//! on every 10-second progress tick, which is inside the 15-second scrape
//! interval. Without a recorder (tests, local runs) `publish` is a no-op.
//!
//! What is deliberately not exported:
//! - `total_deletes`, which nothing increments (entity deletes are counted as
//!   updates by the loader), so it would read 0 forever and look like a signal.
//! - A failed-batch counter. A batch that fails in the processor or loader is
//!   NACKed, and a NACK shuts the consumer and then the process down so that
//!   Kafka redelivers it. A counter would die with the process before a scrape
//!   could read it; the container restart count is the signal for that path.
//! - A position gauge. The topics only carry messages when someone writes, so a
//!   "last processed" value would stall through quiet stretches. Per-partition
//!   lag is already watched by kafka-exporter.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub const EVENTS_PROCESSED: &str = "search_indexer_events_processed_total";
pub const DOCUMENTS_INDEXED: &str = "search_indexer_documents_indexed_total";
pub const BULK_CALLS: &str = "search_indexer_bulk_calls_total";
pub const BULK_WALL_MS: &str = "search_indexer_bulk_wall_milliseconds_total";
pub const BULK_TOOK_MS: &str = "search_indexer_bulk_took_milliseconds_total";
pub const OPERATIONS: &str = "search_indexer_operations_total";
pub const OPERATIONS_FAILED: &str = "search_indexer_operations_failed_total";
pub const OPERATIONS_BY_KIND: &str = "search_indexer_operations_by_kind_total";
pub const CANONICAL_GRAPH_NODES: &str = "search_indexer_canonical_graph_nodes";
pub const RETIRE_DROPPED: &str = "search_indexer_retire_candidates_dropped_total";
pub const RETIRE_PENDING: &str = "search_indexer_retire_candidates_pending";

/// Metrics for the search indexer orchestrator.
#[derive(Debug)]
pub struct SearchIndexerMetrics {
    /// Total number of events processed since startup.
    pub total_events_processed: Arc<AtomicU64>,
    /// Total number of documents indexed since startup.
    pub total_documents_indexed: Arc<AtomicU64>,

    // Bulk call metrics
    /// Number of execute_bulk + update_by_query calls.
    pub total_bulk_calls: Arc<AtomicU64>,
    /// Cumulative wall-clock ms for all bulk calls.
    pub total_bulk_wall_ms: Arc<AtomicU64>,
    /// Cumulative server-side took ms.
    pub total_bulk_took_ms: Arc<AtomicU64>,
    /// Total individual operations sent to OpenSearch.
    pub total_operations: Arc<AtomicU64>,
    /// Total failed individual operations.
    pub total_failed_operations: Arc<AtomicU64>,

    // Operation type counts
    /// Update/upsert operations (Index + AddRelation).
    pub total_updates: Arc<AtomicU64>,
    /// Delete operations.
    pub total_deletes: Arc<AtomicU64>,
    /// Unset property operations.
    pub total_unsets: Arc<AtomicU64>,
    /// Remove relation by ID operations.
    pub total_remove_relations: Arc<AtomicU64>,
    /// Score update operations (all 3 score types combined).
    pub total_score_updates: Arc<AtomicU64>,
    /// Space topic entity ID update operations.
    pub total_space_topic_updates: Arc<AtomicU64>,
    /// Current number of nodes in the canonical topology graph.
    pub canonical_graph_size: Arc<AtomicU64>,

    // Emptied-document retirement (GEO-2548)
    /// Retirements sent to OpenSearch that did not fail (a no-op on a tombstone or topic stub,
    /// or a document already gone, counts too).
    pub total_retires: Arc<AtomicU64>,
    /// Candidates dropped because kg-indexer never reached their block within the TTL.
    pub retire_expired: Arc<AtomicU64>,
    /// Candidates refused because the tracker was full.
    pub retire_overflow: Arc<AtomicU64>,
    /// Deleted relations whose document could not be found in Postgres.
    pub retire_unresolved: Arc<AtomicU64>,
    /// Candidates currently waiting for kg-indexer.
    pub retire_pending: Arc<AtomicU64>,
}

impl SearchIndexerMetrics {
    /// Create a new metrics instance with all counters initialized to zero.
    pub fn new() -> Self {
        Self {
            total_events_processed: Arc::new(AtomicU64::new(0)),
            total_documents_indexed: Arc::new(AtomicU64::new(0)),
            total_bulk_calls: Arc::new(AtomicU64::new(0)),
            total_bulk_wall_ms: Arc::new(AtomicU64::new(0)),
            total_bulk_took_ms: Arc::new(AtomicU64::new(0)),
            total_operations: Arc::new(AtomicU64::new(0)),
            total_failed_operations: Arc::new(AtomicU64::new(0)),
            total_updates: Arc::new(AtomicU64::new(0)),
            total_deletes: Arc::new(AtomicU64::new(0)),
            total_unsets: Arc::new(AtomicU64::new(0)),
            total_remove_relations: Arc::new(AtomicU64::new(0)),
            total_score_updates: Arc::new(AtomicU64::new(0)),
            total_space_topic_updates: Arc::new(AtomicU64::new(0)),
            canonical_graph_size: Arc::new(AtomicU64::new(0)),
            total_retires: Arc::new(AtomicU64::new(0)),
            retire_expired: Arc::new(AtomicU64::new(0)),
            retire_overflow: Arc::new(AtomicU64::new(0)),
            retire_unresolved: Arc::new(AtomicU64::new(0)),
            retire_pending: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl SearchIndexerMetrics {
    /// Describe every exported series. Call once, after the recorder is installed
    /// and before the first [`publish`](Self::publish).
    pub fn describe() {
        metrics::describe_counter!(
            EVENTS_PROCESSED,
            "Kafka events the processor handled (edits, scores, space topics, topology)"
        );
        metrics::describe_counter!(
            DOCUMENTS_INDEXED,
            "Entity documents written to OpenSearch in batches that fully succeeded"
        );
        metrics::describe_counter!(BULK_CALLS, "OpenSearch bulk and update_by_query calls");
        metrics::describe_counter!(
            BULK_WALL_MS,
            "Wall-clock milliseconds spent in OpenSearch bulk calls, measured by the indexer"
        );
        metrics::describe_counter!(
            BULK_TOOK_MS,
            "Server-side milliseconds OpenSearch reported (`took`) for bulk calls"
        );
        metrics::describe_counter!(
            OPERATIONS,
            "Individual operations sent to OpenSearch inside bulk calls"
        );
        metrics::describe_counter!(
            OPERATIONS_FAILED,
            "Individual OpenSearch operations that failed. Any failure NACKs its batch, \
             which shuts the indexer down so the batch is redelivered after the restart"
        );
        metrics::describe_counter!(
            OPERATIONS_BY_KIND,
            "Processed events handed to the loader, by kind"
        );
        metrics::describe_gauge!(
            CANONICAL_GRAPH_NODES,
            "Spaces in the in-memory canonical topology graph"
        );
        metrics::describe_counter!(
            RETIRE_DROPPED,
            "Emptied-document candidates dropped unchecked, by reason: expired (kg-indexer \
             never reached their block), overflow (tracker full), unresolved (deleted \
             relation not found in Postgres). The daily orphan reconcile catches these"
        );
        metrics::describe_gauge!(
            RETIRE_PENDING,
            "Emptied-document candidates waiting for kg-indexer to reach their block"
        );
    }

    /// Copy every counter into the Prometheus recorder.
    ///
    /// `absolute` sets a counter to the atomic's current value. The atomics only
    /// grow, so the exported counters stay monotonic.
    pub fn publish(&self) {
        let get = |a: &AtomicU64| a.load(Ordering::Relaxed);

        metrics::counter!(EVENTS_PROCESSED).absolute(get(&self.total_events_processed));
        metrics::counter!(DOCUMENTS_INDEXED).absolute(get(&self.total_documents_indexed));
        metrics::counter!(BULK_CALLS).absolute(get(&self.total_bulk_calls));
        metrics::counter!(BULK_WALL_MS).absolute(get(&self.total_bulk_wall_ms));
        metrics::counter!(BULK_TOOK_MS).absolute(get(&self.total_bulk_took_ms));
        metrics::counter!(OPERATIONS).absolute(get(&self.total_operations));
        metrics::counter!(OPERATIONS_FAILED).absolute(get(&self.total_failed_operations));
        for (kind, value) in [
            ("update", &self.total_updates),
            ("unset", &self.total_unsets),
            ("remove_relation", &self.total_remove_relations),
            ("score_update", &self.total_score_updates),
            ("space_topic_update", &self.total_space_topic_updates),
            ("retire", &self.total_retires),
        ] {
            metrics::counter!(OPERATIONS_BY_KIND, "kind" => kind).absolute(get(value));
        }
        metrics::gauge!(CANONICAL_GRAPH_NODES).set(get(&self.canonical_graph_size) as f64);
        for (reason, value) in [
            ("expired", &self.retire_expired),
            ("overflow", &self.retire_overflow),
            ("unresolved", &self.retire_unresolved),
        ] {
            metrics::counter!(RETIRE_DROPPED, "reason" => reason).absolute(get(value));
        }
        metrics::gauge!(RETIRE_PENDING).set(get(&self.retire_pending) as f64);
    }
}

impl Default for SearchIndexerMetrics {
    fn default() -> Self {
        Self::new()
    }
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
    fn publish_exposes_every_series_at_zero() {
        let m = SearchIndexerMetrics::new();
        let rendered = render_with(|| {
            SearchIndexerMetrics::describe();
            m.publish();
        });
        for line in [
            "search_indexer_events_processed_total 0",
            "search_indexer_documents_indexed_total 0",
            "search_indexer_bulk_calls_total 0",
            "search_indexer_bulk_wall_milliseconds_total 0",
            "search_indexer_bulk_took_milliseconds_total 0",
            "search_indexer_operations_total 0",
            "search_indexer_operations_failed_total 0",
            "search_indexer_operations_by_kind_total{kind=\"update\"} 0",
            "search_indexer_operations_by_kind_total{kind=\"space_topic_update\"} 0",
            "search_indexer_canonical_graph_nodes 0",
        ] {
            assert!(rendered.contains(line), "missing `{line}` in:\n{rendered}");
        }
        assert!(
            !rendered.contains("kind=\"delete\""),
            "total_deletes is never incremented and must not be exported:\n{rendered}"
        );
    }

    #[test]
    fn publish_follows_the_atomics() {
        let m = SearchIndexerMetrics::new();
        let rendered = render_with(|| {
            m.publish();
            m.total_events_processed.fetch_add(40, Ordering::Relaxed);
            m.total_failed_operations.fetch_add(2, Ordering::Relaxed);
            m.total_score_updates.fetch_add(7, Ordering::Relaxed);
            m.canonical_graph_size.store(1234, Ordering::Relaxed);
            m.publish();
            m.total_events_processed.fetch_add(2, Ordering::Relaxed);
            m.publish();
        });
        for line in [
            "search_indexer_events_processed_total 42",
            "search_indexer_operations_failed_total 2",
            "search_indexer_operations_by_kind_total{kind=\"score_update\"} 7",
            "search_indexer_canonical_graph_nodes 1234",
        ] {
            assert!(rendered.contains(line), "missing `{line}` in:\n{rendered}");
        }
    }
}
