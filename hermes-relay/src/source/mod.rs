//! Mock block data and event builders for testing.
//!
//! Event builders mirror mock-substream's event types:
//! - `space_created` / `space_created_dao` → SpaceCreated
//! - `trust_extended_verified` / `_related` / `_subtopic` → TrustExtended
//! - `edit_published` → EditPublished
//!
//! # Example
//!
//! ```ignore
//! use hermes_relay::source::{MockSource, mock_events};
//! use hermes_substream::pb::hermes::Actions;
//! use prost::Message;
//!
//! // Create specific events
//! let actions = Actions {
//!     actions: vec![
//!         mock_events::space_created([0x01; 16], [0xaa; 32]),
//!         mock_events::trust_extended_verified([0x01; 16], [0x02; 16]),
//!         mock_events::edit_published([0x01; 16], "QmYwAPJzv5CZsnA..."),
//!     ],
//! };
//!
//! let source = MockSource::builder(actions.encode_to_vec()).with_blocks(100, 110);
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
