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
}

impl SearchIndexerMetrics {
    /// Create a new metrics instance with all counters initialized to zero.
    pub fn new() -> Self {
        Self {
            total_events_processed: Arc::new(AtomicU64::new(0)),
            total_documents_indexed: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl Default for SearchIndexerMetrics {
    fn default() -> Self {
        Self::new()
    }
}

