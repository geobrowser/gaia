//! Hermes Pipeline library
//!
//! This module exports the core functionality of the Hermes Pipeline for use in
//! integration tests and other consumers.

mod cache;
pub mod decode;
pub mod pipelines;

// Re-export commonly used types
pub use decode::{ProposalActionType, decode_address_arg};
pub use pipelines::BlockMetadata;
