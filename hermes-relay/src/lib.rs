//! Hermes Relay
//!
//! Shared library for connecting to the hermes-substream blockchain data source.
//!
//! This crate provides:
//! - Re-exports of `stream` crate infrastructure (SubstreamsEndpoint, Sink traits, etc.)
//! - Hermes-specific configuration (module names, package paths)
//! - Action type constants for filtering raw actions
//!
//! ## Usage
//!
//! Transformers implement the `Sink` or `PreprocessedSink` trait and use the
//! hermes configuration to connect to the appropriate substream module:
//!
//! ```ignore
//! use hermes_relay::{Sink, HermesModule, HERMES_SPKG};
//!
//! struct EditsTransformer { /* ... */ }
//!
//! impl Sink<EditData> for EditsTransformer {
//!     // Implement cursor persistence and block processing...
//! }
//!
//! // Run the transformer with a specific module
//! let module = HermesModule::EditsPublished;
//! transformer.run(
//!     &endpoint_url,
//!     HERMES_SPKG,
//!     module.as_str(),
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
//! use hermes_relay::{Sink, HermesModule, HERMES_SPKG, actions};
//!
//! // Spaces transformer needs: SpacesRegistered, SubspacesAdded, SubspacesRemoved
//! // Use HermesModule::Actions and filter by action type
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

// Re-export config types at crate root for convenience
pub use config::{HermesModule, HERMES_SPKG};

// Re-export stream crate types for transformers
pub use stream::{
    pb, // Substreams protobuf types
    substreams::SubstreamsEndpoint,
    substreams_stream::{BlockResponse, SubstreamsStream},
    utils, // Block metadata utilities
    PreprocessedSink,
    Sink,
};
