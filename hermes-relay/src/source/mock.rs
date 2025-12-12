//! Mock implementation of BlockSource for testing.
//!
//! `MockSource` replays pre-built [`BlockData`] for testing transformers without
//! a real substream connection. The block data should contain protobuf-encoded
//! output using the real `hermes_substream::pb::hermes::*` types.
//!
//! # Example
//!
//! ```ignore
//! use hermes_relay::source::{BlockData, MockSource};
//! use hermes_substream::pb::hermes::{Action, Actions};
//! use prost::Message;
//!
//! // Build actions using real protobuf types
//! let actions = Actions {
//!     actions: vec![
//!         Action {
//!             from_id: vec![0; 16],
//!             to_id: vec![0; 16],
//!             action: hermes_relay::actions::SPACE_REGISTERED.to_vec(),
//!             topic: vec![0; 32],
//!             data: vec![],
//!         },
//!     ],
//! };
//!
//! // Create block with encoded protobuf
//! let block = BlockData {
//!     block_number: 1000,
//!     timestamp: 1234567890,
//!     cursor: "cursor_1".to_string(),
//!     output: actions.encode_to_vec(),
//!     module_name: "map_actions".to_string(),
//! };
//!
//! // Create source and run transformer
//! let source = MockSource::new(vec![block]);
//! transformer.run_with_source(source).await?;
//! ```

use std::collections::VecDeque;

use async_trait::async_trait;

use super::{BlockData, BlockResponse, BlockSource};

/// A mock block source that replays pre-built blocks.
///
/// This is useful for testing transformers with deterministic data without
/// requiring a real substream connection.
pub struct MockSource {
    blocks: VecDeque<BlockData>,
    current_cursor: Option<String>,
}

impl MockSource {
    /// Create a new mock source from pre-built blocks.
    ///
    /// Blocks are consumed in order. The source is exhausted when all blocks
    /// have been yielded.
    pub fn new(blocks: Vec<BlockData>) -> Self {
        Self {
            blocks: blocks.into(),
            current_cursor: None,
        }
    }

    /// Create a mock source that resumes from a cursor position.
    ///
    /// Skips blocks until the cursor is found, then begins emitting from the
    /// next block. This is useful for testing cursor persistence and restart
    /// scenarios.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let blocks = vec![block1, block2, block3];
    ///
    /// // Resume from block1's cursor - will emit block2 and block3
    /// let source = MockSource::resume_from(blocks, &block1.cursor);
    /// ```
    pub fn resume_from(blocks: Vec<BlockData>, cursor: &str) -> Self {
        let mut source = Self::new(blocks);

        // Skip blocks until we find the cursor
        while let Some(block) = source.blocks.front() {
            if block.cursor == cursor {
                // Found the cursor - remove this block (already processed)
                // and start from the next one
                source.blocks.pop_front();
                source.current_cursor = Some(cursor.to_string());
                break;
            }
            source.blocks.pop_front();
        }

        source
    }

    /// Returns the number of remaining blocks.
    pub fn remaining(&self) -> usize {
        self.blocks.len()
    }
}

#[async_trait]
impl BlockSource for MockSource {
    async fn next(&mut self) -> Option<Result<BlockResponse, anyhow::Error>> {
        let block = self.blocks.pop_front()?;
        self.current_cursor = Some(block.cursor.clone());

        Some(Ok(BlockResponse::New(block)))
    }

    fn cursor(&self) -> Option<&str> {
        self.current_cursor.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_block(number: u64, cursor: &str) -> BlockData {
        BlockData {
            block_number: number,
            timestamp: 1000 + number,
            cursor: cursor.to_string(),
            output: vec![],
            module_name: "test".to_string(),
        }
    }

    #[tokio::test]
    async fn test_mock_source_yields_blocks_in_order() {
        let blocks = vec![
            make_block(1, "cursor_1"),
            make_block(2, "cursor_2"),
            make_block(3, "cursor_3"),
        ];

        let mut source = MockSource::new(blocks);

        let block1 = source.next().await.unwrap().unwrap();
        assert!(matches!(block1, BlockResponse::New(b) if b.block_number == 1));

        let block2 = source.next().await.unwrap().unwrap();
        assert!(matches!(block2, BlockResponse::New(b) if b.block_number == 2));

        let block3 = source.next().await.unwrap().unwrap();
        assert!(matches!(block3, BlockResponse::New(b) if b.block_number == 3));

        assert!(source.next().await.is_none());
    }

    #[tokio::test]
    async fn test_mock_source_updates_cursor() {
        let blocks = vec![make_block(1, "cursor_1"), make_block(2, "cursor_2")];

        let mut source = MockSource::new(blocks);
        assert!(source.cursor().is_none());

        source.next().await;
        assert_eq!(source.cursor(), Some("cursor_1"));

        source.next().await;
        assert_eq!(source.cursor(), Some("cursor_2"));
    }

    #[tokio::test]
    async fn test_mock_source_resume_from_cursor() {
        let blocks = vec![
            make_block(1, "cursor_1"),
            make_block(2, "cursor_2"),
            make_block(3, "cursor_3"),
        ];

        let mut source = MockSource::resume_from(blocks, "cursor_1");

        // Should skip block 1 and start from block 2
        assert_eq!(source.remaining(), 2);
        assert_eq!(source.cursor(), Some("cursor_1"));

        let block = source.next().await.unwrap().unwrap();
        assert!(matches!(block, BlockResponse::New(b) if b.block_number == 2));
    }

    #[tokio::test]
    async fn test_mock_source_resume_from_unknown_cursor() {
        let blocks = vec![make_block(1, "cursor_1"), make_block(2, "cursor_2")];

        let mut source = MockSource::resume_from(blocks, "unknown");

        // Should exhaust all blocks looking for cursor
        assert_eq!(source.remaining(), 0);
        assert!(source.next().await.is_none());
    }
}
