//! Processor module for the search indexer ingest.
//!
//! Transforms entity events into search documents.

mod entity_processor;

pub use entity_processor::{EntityProcessor, ProcessedEvent};
