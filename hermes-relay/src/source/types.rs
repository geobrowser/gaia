//! Types for the BlockSource abstraction.
//!
//! These types provide a unified interface for block data from any source
//! (real substreams or mock data).

use stream::pb::sf::substreams::{
    rpc::v2::{BlockScopedData, BlockUndoSignal, MapModuleOutput},
    v1::{BlockRef, Clock},
};

/// A block of data from any source (real or mock).
///
/// This struct provides a unified representation of block data that can be
/// constructed from either a real substream response or mock data.
#[derive(Debug, Clone)]
pub struct BlockData {
    /// The block number.
    pub block_number: u64,
    /// Unix timestamp in seconds.
    pub timestamp: u64,
    /// Cursor for resuming from this point.
    pub cursor: String,
    /// Module output data (protobuf-encoded).
    pub output: Vec<u8>,
    /// Module name that produced this output.
    pub module_name: String,
}

impl BlockData {
    /// Convert this BlockData to a BlockScopedData for compatibility with existing Sink methods.
    ///
    /// This allows transformers to continue using their existing `process_block_scoped_data`
    /// implementation while consuming from any BlockSource.
    pub fn to_block_scoped_data(&self) -> BlockScopedData {
        use prost_types::Timestamp;

        BlockScopedData {
            cursor: self.cursor.clone(),
            final_block_height: self.block_number,
            clock: Some(Clock {
                id: String::new(),
                number: self.block_number,
                timestamp: Some(Timestamp {
                    seconds: self.timestamp as i64,
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

/// Signal to undo blocks due to chain reorganization.
#[derive(Debug, Clone)]
pub struct UndoSignal {
    /// The last valid block number after the reorg.
    pub last_valid_block: u64,
    /// The cursor for the last valid block.
    pub last_valid_cursor: String,
}

impl UndoSignal {
    /// Convert this UndoSignal to a BlockUndoSignal for compatibility with existing Sink methods.
    pub fn to_block_undo_signal(&self) -> BlockUndoSignal {
        BlockUndoSignal {
            last_valid_cursor: self.last_valid_cursor.clone(),
            last_valid_block: Some(BlockRef {
                id: String::new(),
                number: self.last_valid_block,
            }),
        }
    }
}

/// Response from a block source.
#[derive(Debug, Clone)]
pub enum BlockResponse {
    /// A new block of data.
    New(BlockData),
    /// Signal to undo blocks due to chain reorganization.
    Undo(UndoSignal),
}
