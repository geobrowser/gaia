//! Block source abstraction for consuming events from real or mock substreams.
//!
//! This module provides the [`BlockSource`] trait that abstracts over the event source,
//! allowing both real substreams and mock data to be consumed through the same interface.
//!
//! # Usage
//!
//! Transformers can use `Sink::run_with_source()` to consume from any [`BlockSource`]:
//!
//! ```ignore
//! use hermes_relay::{Sink, HermesModule};
//! use hermes_relay::source::MockSource;
//! use hermes_substream::pb::hermes::Actions;
//! use prost::Message;
//!
//! // Build mock data using real protobuf types
//! let actions = Actions {
//!     actions: vec![/* ... */],
//! };
//!
//! // Create mock source with encoded protobuf output
//! let source = MockSource::new(actions.encode_to_vec())
//!     .with_blocks(1000, 1005);  // blocks 1000-1004
//!
//! transformer.run_with_source(source).await?;
//! ```

mod mock;

pub use mock::MockSource;

use async_trait::async_trait;
use stream::substreams_stream::BlockResponse;

/// Trait for consuming blocks from any source.
///
/// This trait abstracts over the event source, allowing both real substreams
/// and mock data to be consumed through the same interface.
///
/// The `BlockResponse` type is the same one used by the real substream,
/// containing `BlockScopedData` for new blocks and `BlockUndoSignal` for reorgs.
#[async_trait]
pub trait BlockSource: Send {
    /// Get the next block response, or None if the stream is exhausted.
    async fn next(&mut self) -> Option<Result<BlockResponse, anyhow::Error>>;

    /// Get the current cursor position.
    fn cursor(&self) -> Option<&str>;
}
