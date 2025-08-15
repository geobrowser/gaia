use crate::pb::sf::substreams::rpc::v2::BlockScopedData;

pub fn output(block_data: &BlockScopedData) -> &prost_types::Any {
    return block_data
        .output
        .as_ref()
        .unwrap()
        .map_output
        .as_ref()
        .unwrap();
}

#[derive(Clone, Debug)]
pub struct BlockMetadata {
    pub cursor: String,
    pub block_number: u64,
    pub timestamp: String,
}

pub fn block_metadata(block_data: &BlockScopedData) -> BlockMetadata {
    let clock = block_data.clock.as_ref().unwrap();
    let timestamp = clock.timestamp.as_ref().unwrap();

    return BlockMetadata {
        timestamp: timestamp.seconds.to_string(),
        block_number: clock.number,
        cursor: block_data.cursor.clone(),
    };
}
