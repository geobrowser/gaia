//! Mock implementation of BlockSource for testing.
//!
//! `MockSource` produces `BlockScopedData` for testing transformers without
//! a real substream connection. The output should be protobuf-encoded using
//! the real `hermes_substream::pb::hermes::*` types.
//!
//! # Example
//!
//! ```ignore
//! use hermes_relay::source::MockSource;
//! use hermes_substream::pb::hermes::{Action, Actions};
//! use hermes_relay::actions;
//! use prost::Message;
//!
//! // Build actions using real protobuf types
//! let actions = Actions {
//!     actions: vec![
//!         Action {
//!             from_id: vec![0; 16],
//!             to_id: vec![0; 16],
//!             action: actions::SPACE_REGISTERED.to_vec(),
//!             topic: vec![0; 32],
//!             data: vec![],
//!         },
//!     ],
//! };
//!
//! // Create source - each block will contain the same encoded output
//! let source = MockSource::new(actions.encode_to_vec())
//!     .with_blocks(1000, 1003);  // 3 blocks: 1000, 1001, 1002
//!
//! transformer.run_with_source(source).await?;
//! ```

use async_trait::async_trait;
use stream::pb::sf::substreams::{rpc::v2::BlockScopedData, v1::Clock};
use stream::substreams_stream::BlockResponse;

use super::BlockSource;

/// A mock block source for testing.
///
/// Generates `BlockScopedData` with the provided protobuf-encoded output.
pub struct MockSource {
    /// Pre-built blocks to emit.
    blocks: Vec<BlockScopedData>,
    /// Current position in blocks.
    position: usize,
    /// Current cursor.
    current_cursor: Option<String>,
}

impl MockSource {
    /// Create a new mock source that will emit blocks with the given output.
    ///
    /// Call `with_blocks()` or `with_block_data()` to configure which blocks to emit.
    pub fn new(output: Vec<u8>) -> MockSourceBuilder {
        MockSourceBuilder {
            output,
            module_name: "map_actions".to_string(),
        }
    }

    /// Create a mock source from pre-built `BlockScopedData`.
    ///
    /// Use this when you need full control over the block structure.
    pub fn from_blocks(blocks: Vec<BlockScopedData>) -> Self {
        Self {
            blocks,
            position: 0,
            current_cursor: None,
        }
    }

    /// Returns the number of remaining blocks.
    pub fn remaining(&self) -> usize {
        self.blocks.len().saturating_sub(self.position)
    }
}

/// Builder for creating a MockSource with configured blocks.
pub struct MockSourceBuilder {
    output: Vec<u8>,
    module_name: String,
}

impl MockSourceBuilder {
    /// Set the module name for the output.
    pub fn with_module_name(mut self, name: &str) -> Self {
        self.module_name = name.to_string();
        self
    }

    /// Generate blocks for a range of block numbers.
    ///
    /// Each block will have the same output data.
    /// Timestamps are generated as `block_number * 12` (simulating 12-second blocks).
    /// Cursors are generated as `"cursor_{block_number}"`.
    pub fn with_blocks(self, start_block: u64, end_block: u64) -> MockSource {
        let blocks = (start_block..end_block)
            .map(|block_number| self.build_block(block_number))
            .collect();

        MockSource {
            blocks,
            position: 0,
            current_cursor: None,
        }
    }

    /// Generate a single block.
    pub fn single_block(self, block_number: u64) -> MockSource {
        self.with_blocks(block_number, block_number + 1)
    }

    /// Build a BlockScopedData for the given block number.
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

#[async_trait]
impl BlockSource for MockSource {
    async fn next(&mut self) -> Option<Result<BlockResponse, anyhow::Error>> {
        if self.position >= self.blocks.len() {
            return None;
        }

        let block = self.blocks[self.position].clone();
        self.current_cursor = Some(block.cursor.clone());
        self.position += 1;

        Some(Ok(BlockResponse::New(block)))
    }

    fn cursor(&self) -> Option<&str> {
        self.current_cursor.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_source_yields_blocks_in_order() {
        let mut source = MockSource::new(vec![1, 2, 3]).with_blocks(100, 103);

        let block1 = source.next().await.unwrap().unwrap();
        assert!(matches!(block1, BlockResponse::New(b) if b.clock.as_ref().unwrap().number == 100));

        let block2 = source.next().await.unwrap().unwrap();
        assert!(matches!(block2, BlockResponse::New(b) if b.clock.as_ref().unwrap().number == 101));

        let block3 = source.next().await.unwrap().unwrap();
        assert!(matches!(block3, BlockResponse::New(b) if b.clock.as_ref().unwrap().number == 102));

        assert!(source.next().await.is_none());
    }

    #[tokio::test]
    async fn test_mock_source_updates_cursor() {
        let mut source = MockSource::new(vec![]).with_blocks(100, 102);
        assert!(source.cursor().is_none());

        source.next().await;
        assert_eq!(source.cursor(), Some("cursor_100"));

        source.next().await;
        assert_eq!(source.cursor(), Some("cursor_101"));
    }

    #[tokio::test]
    async fn test_mock_source_remaining() {
        let mut source = MockSource::new(vec![]).with_blocks(0, 3);
        assert_eq!(source.remaining(), 3);

        source.next().await;
        assert_eq!(source.remaining(), 2);

        source.next().await;
        source.next().await;
        assert_eq!(source.remaining(), 0);
    }

    #[tokio::test]
    async fn test_mock_source_single_block() {
        let mut source = MockSource::new(vec![42]).single_block(999);

        let block = source.next().await.unwrap().unwrap();
        match block {
            BlockResponse::New(data) => {
                assert_eq!(data.clock.as_ref().unwrap().number, 999);
                assert_eq!(
                    data.output.as_ref().unwrap().map_output.as_ref().unwrap().value,
                    vec![42]
                );
            }
            _ => panic!("expected New block"),
        }

        assert!(source.next().await.is_none());
    }

    #[tokio::test]
    async fn test_mock_source_from_blocks() {
        use stream::pb::sf::substreams::rpc::v2::MapModuleOutput;

        let custom_block = BlockScopedData {
            cursor: "custom_cursor".to_string(),
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

        let mut source = MockSource::from_blocks(vec![custom_block]);

        let block = source.next().await.unwrap().unwrap();
        match block {
            BlockResponse::New(data) => {
                assert_eq!(data.cursor, "custom_cursor");
                assert_eq!(data.output.as_ref().unwrap().name, "custom_module");
            }
            _ => panic!("expected New block"),
        }
    }
}
