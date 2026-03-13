//! Metrics module for the search indexer orchestrator.
//!
//! Provides metrics tracking for events processed and documents indexed.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

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
        }
    }
}

impl Default for SearchIndexerMetrics {
    fn default() -> Self {
        Self::new()
    }
}
