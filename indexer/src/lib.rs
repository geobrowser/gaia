use cache::PreprocessedEdit;
use stream::utils::BlockMetadata;

pub mod block_handler;
pub mod cache;
pub mod error;
pub mod models;
pub mod storage;
pub mod validators;

pub struct KgData {
    pub block: BlockMetadata,
    pub edits: Vec<PreprocessedEdit>,
}
