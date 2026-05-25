//! Hermes Pipeline library
//!
//! This module exports the core functionality of the Hermes Pipeline for use in
//! integration tests and other consumers.

pub mod cache;
pub mod cursor;
pub mod decode;
pub mod pipelines;

// Re-export commonly used types
pub use decode::{
    FlagArgs, PingArgs, ProposalActionType, PublishArgs, VotingSettingsArgs, decode_flag_args,
    decode_ping_args, decode_publish_args, decode_space_id_arg, decode_voting_settings_args,
};
pub use pipelines::BlockMetadata;
