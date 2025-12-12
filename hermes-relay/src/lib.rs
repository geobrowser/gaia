//! Hermes Relay
//!
//! Shared library for connecting to the hermes-substream blockchain data source.
//!
//! This crate provides:
//! - [`Sink`] and [`PreprocessedSink`] traits for consuming hermes-substream events
//! - Hermes-specific configuration (module names, package paths)
//! - Action type constants for filtering raw actions
//!
//! ## Usage
//!
//! Transformers implement the [`Sink`] trait and call `run` with a type-safe
//! [`HermesModule`]:
//!
//! ```ignore
//! use hermes_relay::{Sink, HermesModule};
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
//!     
//!     // Implement cursor persistence...
//! }
//!
//! // Run with type-safe module selection
//! let transformer = EditsTransformer { /* ... */ };
//! transformer.run(
//!     &endpoint_url,
//!     HermesModule::EditsPublished,
//!     start_block,
//!     end_block,
//! ).await?;
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
//!
//! See `docs/decisions/0001-multiple-substreams-modules-consumers.md` for more details.

pub mod actions;
pub mod config;
pub mod sink;

// Re-export config types at crate root for convenience
pub use config::{HermesModule, HERMES_SPKG};

// Re-export sink traits
pub use sink::{PreprocessedSink, Sink};

// Re-export hermes-substream types for consumers
pub use hermes_substream::pb::hermes::{Action, Actions};

// Re-export stream crate for consumers who need substreams types
pub use stream;
