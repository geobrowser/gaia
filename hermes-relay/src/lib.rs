//! Hermes Relay
//!
//! Shared library for connecting to the hermes-substream blockchain data source.
//!
//! This crate provides:
//! - [`Sink`] and [`PreprocessedSink`] traits for consuming hermes-substream events
//! - [`StreamSource`] config for explicitly choosing mock or live data sources
//! - [`source::MockSource`] and [`source::mock_events`] for custom mock data
//! - Hermes-specific configuration (module names, package paths)
//! - Action type constants for filtering raw actions
//!
//! ## Usage
//!
//! Transformers implement the [`Sink`] trait and call `run` with a [`StreamSource`]:
//!
//! ```ignore
//! use hermes_relay::{Sink, StreamSource, HermesModule};
//! use stream::pb::sf::substreams::rpc::v2::BlockScopedData;
//!
//! struct EditsTransformer { /* ... */ }
//!
//! impl Sink for EditsTransformer {
//!     type Error = anyhow::Error;
//!     
//!     async fn process_block_scoped_data(&self, data: &BlockScopedData) -> Result<(), Self::Error> {
//!         // Process the block...
//!         Ok(())
//!     }
//! }
//!
//! // Development/testing: use mock data (all events in a single block)
//! transformer.run(StreamSource::mock()).await?;
//!
//! // Production: use live substream
//! let source = StreamSource::live(
//!     "https://substreams.example.com",
//!     HermesModule::EditsPublished,
//!     0,
//!     1000,
//! );
//! transformer.run(source).await?;
//! ```
//!
//! ## Mock Data Testing
//!
//! For more control over mock data, use [`source::MockSource`] directly:
//!
//! ```ignore
//! use hermes_relay::source::{MockSource, mock_events};
//! use hermes_substream::pb::hermes::Actions;
//! use prost::Message;
//!
//! // Create mock actions
//! let actions = Actions {
//!     actions: vec![
//!         mock_events::space_created([0x01; 16], [0xaa; 32]),
//!         mock_events::trust_extended_verified([0x01; 16], [0x02; 16]),
//!         mock_events::edit_published([0x01; 16], "QmYwAPJzv5CZsnA..."),
//!     ],
//! };
//!
//! // Iterate over mock blocks and process directly
//! for block in MockSource::builder(actions.encode_to_vec()).with_blocks(100, 110) {
//!     transformer.process_block_scoped_data(&block).await?;
//! }
//!
//! // Or use the full test topology matching mock-substream
//! for block in MockSource::test_topology().with_blocks(100, 150) {
//!     transformer.process_block_scoped_data(&block).await?;
//! }
//! ```
//!
//! ## Single vs Multiple Event Types
//!
//! For transformers that need a **single event type**, use the specific module
//! (e.g., `HermesModule::EditsPublished`, `HermesModule::SpacesRegistered`).
//!
//! For transformers that need **multiple event types**, use `HermesModule::Actions`
//! and filter client-side using the constants in the [`actions`] module:
//!
//! ```ignore
//! use hermes_relay::{actions, HermesModule};
//!
//! fn is_space_event(action_bytes: &[u8]) -> bool {
//!     actions::matches(action_bytes, &actions::SPACE_REGISTERED)
//!         || actions::matches(action_bytes, &actions::SUBSPACE_ADDED)
//!         || actions::matches(action_bytes, &actions::SUBSPACE_REMOVED)
//! }
//! ```

pub mod actions;
pub mod config;
pub mod sink;
pub mod source;

// Re-export config types at crate root for convenience
pub use config::{HermesModule, HERMES_SPKG};

// Re-export sink traits and stream source config
pub use sink::{PreprocessedSink, Sink, StreamSource};

// Re-export hermes-substream types for consumers
pub use hermes_substream::pb::hermes::{Action, Actions};

// Re-export stream crate for consumers who need substreams types
pub use stream;
