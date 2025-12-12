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
use futures03::StreamExt;
use stream::substreams_stream::{BlockResponse, SubstreamsStream};

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

/// Wrapper around `SubstreamsStream` that implements `BlockSource`.
pub struct SubstreamSource {
    stream: SubstreamsStream,
    current_cursor: Option<String>,
}

impl SubstreamSource {
    pub fn new(stream: SubstreamsStream, cursor: Option<String>) -> Self {
        Self {
            stream,
            current_cursor: cursor,
        }
    }
}

#[async_trait]
impl BlockSource for SubstreamSource {
    async fn next(&mut self) -> Option<Result<BlockResponse, anyhow::Error>> {
        let result = self.stream.next().await?;

        // Update cursor on successful responses
        if let Ok(ref response) = result {
            match response {
                BlockResponse::New(data) => {
                    self.current_cursor = Some(data.cursor.clone());
                }
                BlockResponse::Undo(signal) => {
                    self.current_cursor = Some(signal.last_valid_cursor.clone());
                }
            }
        }

        Some(result)
    }

    fn cursor(&self) -> Option<&str> {
        self.current_cursor.as_deref()
    }
}
