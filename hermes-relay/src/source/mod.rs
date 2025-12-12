//! Block source abstraction for consuming events from real or mock substreams.
//!
//! This module provides the [`BlockSource`] trait that abstracts over the event source,
//! allowing both real substreams and mock data to be consumed through the same interface.
//!
//! # Architecture
//!
//! ```text
//! Production Path:
//! ┌──────────────┐     ┌─────────────────┐     ┌────────────────┐
//! │  Blockchain  │────▶│ hermes-substream│────▶│  hermes-relay  │
//! └──────────────┘     └─────────────────┘     └────────────────┘
//!                                                      │
//!                                              ┌───────▼───────┐
//!                                              │ SubstreamSource│
//!                                              └───────────────┘
//!
//! Mock Path:
//! ┌───────────────────┐     ┌────────────────┐
//! │ BlockData (mock)  │────▶│  hermes-relay  │
//! └───────────────────┘     └────────────────┘
//!                                   │
//!                           ┌───────▼───────┐
//!                           │  MockSource   │
//!                           └───────────────┘
//! ```
//!
//! # Usage
//!
//! Transformers can use `Sink::run_with_source()` to consume from any [`BlockSource`]:
//!
//! ```ignore
//! use hermes_relay::{Sink, HermesModule};
//! use hermes_relay::source::{BlockSource, BlockData, MockSource};
//! use hermes_substream::pb::hermes::Actions;
//! use prost::Message;
//!
//! // Build mock data using real protobuf types
//! let actions = Actions {
//!     actions: vec![/* ... */],
//! };
//! let block = BlockData {
//!     block_number: 1000,
//!     timestamp: 1234567890,
//!     cursor: "cursor_1".to_string(),
//!     output: actions.encode_to_vec(),
//!     module_name: "map_actions".to_string(),
//! };
//!
//! // Run with mock data
//! let source = MockSource::new(vec![block]);
//! transformer.run_with_source(source).await?;
//!
//! // Or use the production substream
//! let source = SubstreamSource::connect(&endpoint, module, cursor, start, end).await?;
//! transformer.run_with_source(source).await?;
//! ```

mod mock;
mod substream;
mod types;

pub use mock::MockSource;
pub use substream::SubstreamSource;
pub use types::{BlockData, BlockResponse, UndoSignal};

use async_trait::async_trait;

/// Trait for consuming blocks from any source.
///
/// This trait abstracts over the event source, allowing both real substreams
/// and mock data to be consumed through the same interface.
///
/// # Example
///
/// ```ignore
/// use hermes_relay::source::{BlockSource, BlockResponse};
///
/// async fn consume<S: BlockSource>(mut source: S) {
///     while let Some(result) = source.next().await {
///         match result {
///             Ok(BlockResponse::New(data)) => {
///                 println!("Block {}: {} bytes", data.block_number, data.output.len());
///             }
///             Ok(BlockResponse::Undo(signal)) => {
///                 println!("Undo to block {}", signal.last_valid_block);
///             }
///             Err(e) => {
///                 eprintln!("Error: {}", e);
///                 break;
///             }
///         }
///     }
/// }
/// ```
#[async_trait]
pub trait BlockSource: Send {
    /// Get the next block response, or None if the stream is exhausted.
    async fn next(&mut self) -> Option<Result<BlockResponse, anyhow::Error>>;

    /// Get the current cursor position.
    fn cursor(&self) -> Option<&str>;
}
