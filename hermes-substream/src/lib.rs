//! Hermes Substream
//!
//! Decodes raw Ethereum logs into typed events for the Hermes architecture.
//!
//! This crate runs on Substreams infrastructure and emits protobuf-encoded
//! events that are consumed by hermes-relay.

pub mod helpers;
mod pb;

// TODO: Add use_contract! declarations for new ABIs
// use substreams_ethereum::{pb::eth, use_contract, Event};
//
use_contract!(my_contract, "abis/space-registry.json");
