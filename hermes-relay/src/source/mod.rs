//! Mock block data for testing.
//!
//! # Example
//!
//! ```ignore
//! use hermes_relay::source::MockSource;
//! use hermes_substream::pb::hermes::Actions;
//! use prost::Message;
//!
//! let source = MockSource::new(actions.encode_to_vec()).with_blocks(100, 110);
//!
//! for block in source {
//!     sink.process_block_scoped_data(&block).await?;
//! }
//! ```

mod mock;

pub use mock::MockSource;
