//! Unified mock substream event generation for testing.
//!
//! This crate provides a single source of truth for mock blockchain events,
//! allowing both `hermes-producer` and `atlas` to consume the same test data.
//!
//! # Architecture
//!
//! In production, both systems consume events from a real substream:
//!
//! ```text
//! Blockchain → Substreams RPC → ┬→ hermes-producer → Kafka
//!                               └→ Atlas → Kafka
//! ```
//!
//! For testing, this crate provides mock events:
//!
//! ```text
//! MockSubstream → ┬→ hermes-producer → Kafka
//!                 └→ Atlas → Kafka
//! ```
//!
//! # Usage
//!
//! ## Deterministic Testing
//!
//! Use the pre-defined test topology for reproducible tests:
//!
//! ```rust
//! use mock_substream::test_topology;
//!
//! let blocks = test_topology::generate();
//! let canonical = test_topology::canonical_spaces();
//!
//! // Use well-known IDs
//! let root = test_topology::ROOT_SPACE_ID;
//! let space_a = test_topology::SPACE_A;
//! ```
//!
//! ## Custom Event Generation
//!
//! Use the generator for custom scenarios:
//!
//! ```rust
//! use mock_substream::{MockSubstream, MockConfig, MockEvent};
//! use mock_substream::events::*;
//!
//! let mut mock = MockSubstream::deterministic();
//!
//! // Create spaces
//! let space = mock.create_personal_space(
//!     make_id(0x01),
//!     make_id(0x02),
//!     make_address(0xAA),
//! );
//!
//! // Extend trust
//! let trust = mock.extend_verified(make_id(0x01), make_id(0x03));
//!
//! // Get a block with events
//! let block = mock.block_with_events(vec![
//!     MockEvent::SpaceCreated(space),
//!     MockEvent::TrustExtended(trust),
//! ]);
//! ```
//!
//! ## Random Generation (requires `random` feature)
//!
//! ```rust,ignore
//! use mock_substream::{MockSubstream, MockConfig};
//! use rand::thread_rng;
//!
//! let config = MockConfig::default()
//!     .with_num_spaces(20)
//!     .with_edits()
//!     .with_edits_per_space(10);
//!
//! let mut mock = MockSubstream::new(config);
//! let blocks = mock.generate_random_topology(&mut thread_rng());
//! ```
//!
//! # Features
//!
//! - `random`: Enables random event generation using the `rand` crate.

pub mod events;
pub mod generator;
pub mod test_topology;

// Re-export main types at crate root for convenience
pub use events::{
    // ID types
    Address, EditId, EntityId, PropertyId, RelationId, RelationTypeId, SpaceId, TopicId,
    // Block types
    BlockMetadata, MockBlock, MockEvent,
    // Space events
    SpaceCreated, SpaceType,
    // Trust events
    TrustExtended, TrustExtension,
    // Edit events
    EditPublished, Op,
    // Op types
    CreateProperty, CreateRelation, DataType, UnsetEntityValues, UnsetRelationFields,
    UpdateEntity, UpdateRelation, Value,
    // Helpers
    make_address, make_id,
};

pub use generator::{MockConfig, MockSubstream};
