//! Mock block data for testing sink implementations.
//!
//! # Example
//!
//! ```ignore
//! use hermes_relay::source::{MockSource, mock_events};
//! use hermes_substream::pb::hermes::Actions;
//! use prost::Message;
//!
//! // Create with specific events
//! let actions = Actions {
//!     actions: vec![
//!         mock_events::space_created([0x01; 16], [0xaa; 32]),
//!         mock_events::trust_extended_verified([0x01; 16], [0x02; 16]),
//!         mock_events::edit_published([0x01; 16], "QmYwAPJzv5CZsnA..."),
//!     ],
//! };
//! let source = MockSource::builder(actions.encode_to_vec()).with_blocks(100, 105);
//!
//! // Or use test_topology() for the full mock-substream test graph
//! let source = MockSource::test_topology().with_blocks(100, 105);
//!
//! for block in source {
//!     sink.process_block_scoped_data(&block).await?;
//! }
//! ```

use hermes_substream::pb::hermes::Actions;
use prost::Message;
use stream::pb::sf::substreams::{rpc::v2::BlockScopedData, v1::Clock};

use super::mock_events;

/// Creates mock `BlockScopedData` for testing.
pub struct MockSource {
    blocks: Vec<BlockScopedData>,
}

impl MockSource {
    /// Create a builder with the given protobuf-encoded output.
    pub fn builder(output: Vec<u8>) -> MockSourceBuilder {
        MockSourceBuilder {
            output,
            module_name: "map_actions".to_string(),
        }
    }

    /// Create a builder with the full test topology from mock-substream.
    ///
    /// Generates 18 spaces, 19 trust edges, and 6 edits.
    pub fn test_topology() -> MockSourceBuilder {
        let actions = Actions {
            actions: mock_events::test_topology::generate(),
        };
        Self::builder(actions.encode_to_vec())
    }

    /// Create from pre-built blocks.
    pub fn from_blocks(blocks: Vec<BlockScopedData>) -> Self {
        Self { blocks }
    }
}

impl IntoIterator for MockSource {
    type Item = BlockScopedData;
    type IntoIter = std::vec::IntoIter<BlockScopedData>;

    fn into_iter(self) -> Self::IntoIter {
        self.blocks.into_iter()
    }
}

pub struct MockSourceBuilder {
    output: Vec<u8>,
    module_name: String,
}

impl MockSourceBuilder {
    pub fn with_module_name(mut self, name: &str) -> Self {
        self.module_name = name.to_string();
        self
    }

    /// Generate blocks for a range [start_block, end_block).
    pub fn with_blocks(self, start_block: u64, end_block: u64) -> MockSource {
        let blocks = (start_block..end_block)
            .map(|n| self.build_block(n))
            .collect();
        MockSource { blocks }
    }

    pub fn single_block(self, block_number: u64) -> MockSource {
        self.with_blocks(block_number, block_number + 1)
    }

    fn build_block(&self, block_number: u64) -> BlockScopedData {
        use prost_types::Timestamp;
        use stream::pb::sf::substreams::rpc::v2::MapModuleOutput;

        BlockScopedData {
            cursor: format!("cursor_{}", block_number),
            final_block_height: block_number,
            clock: Some(Clock {
                id: format!("block_{}", block_number),
                number: block_number,
                timestamp: Some(Timestamp {
                    seconds: (block_number * 12) as i64,
                    nanos: 0,
                }),
            }),
            output: Some(MapModuleOutput {
                name: self.module_name.clone(),
                map_output: Some(prost_types::Any {
                    type_url: String::new(),
                    value: self.output.clone(),
                }),
                debug_info: None,
            }),
            debug_map_outputs: vec![],
            debug_store_outputs: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_iterates_blocks_in_order() {
        let blocks: Vec<_> = MockSource::builder(vec![1, 2, 3])
            .with_blocks(100, 103)
            .into_iter()
            .collect();

        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].clock.as_ref().unwrap().number, 100);
        assert_eq!(blocks[1].clock.as_ref().unwrap().number, 101);
        assert_eq!(blocks[2].clock.as_ref().unwrap().number, 102);
    }

    #[test]
    fn test_single_block() {
        let blocks: Vec<_> = MockSource::builder(vec![42])
            .single_block(999)
            .into_iter()
            .collect();

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].clock.as_ref().unwrap().number, 999);
        assert_eq!(
            blocks[0]
                .output
                .as_ref()
                .unwrap()
                .map_output
                .as_ref()
                .unwrap()
                .value,
            vec![42]
        );
    }

    #[test]
    fn test_from_blocks() {
        use stream::pb::sf::substreams::rpc::v2::MapModuleOutput;

        let custom = BlockScopedData {
            cursor: "custom".to_string(),
            final_block_height: 500,
            clock: Some(Clock {
                id: "custom".to_string(),
                number: 500,
                timestamp: None,
            }),
            output: Some(MapModuleOutput {
                name: "custom_module".to_string(),
                map_output: None,
                debug_info: None,
            }),
            debug_map_outputs: vec![],
            debug_store_outputs: vec![],
        };

        let blocks: Vec<_> = MockSource::from_blocks(vec![custom]).into_iter().collect();

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].cursor, "custom");
    }
}
