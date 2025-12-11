//! Hermes Relay
//!
//! Shared library for connecting to the blockchain data source.
//!
//! Provides:
//! - Connection setup (currently Substreams, but abstracted)
//! - Cursor/checkpoint management
//! - Event decoding (raw bytes → typed events)
//! - Typed event stream for transformers to consume
