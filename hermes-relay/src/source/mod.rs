//! Mock block data and event builders for testing.
//!
//! Event builders mirror Space Registry contract actions:
//! - `personal_space_registered` / `dao_space_initialized` → SPACE_ID_REGISTERED + SPACE_TYPE_DECLARED
//! - `subspace_verified` / `subspace_related` / `subspace_topic_set` → trust events
//! - `edit_published` → EDITS_PUBLISHED
//!
//! # Example
//!
//! ```ignore
//! use hermes_relay::source::{MockSource, mock_events};
//! use hermes_substream::pb::hermes::Actions;
//! use prost::Message;
//!
//! // Create specific events
//! let mut actions = Vec::new();
//! actions.extend(mock_events::personal_space_registered([0x01; 16], [0xaa; 32]));
//! actions.push(mock_events::subspace_verified([0x01; 16], [0x02; 16]));
//! actions.push(mock_events::edit_published([0x01; 16], "QmYwAPJzv5CZsnA..."));
//!
//! let block = Actions { actions };
//! let source = MockSource::builder(block.encode_to_vec()).with_blocks(100, 110);
//!
//! // Or use the full test topology matching mock-substream
//! let source = MockSource::test_topology().with_blocks(100, 110);
//!
//! for block in source {
//!     sink.process_block_scoped_data(&block).await?;
//! }
//! ```

mod mock;
pub mod mock_events;

pub use mock::MockSource;
