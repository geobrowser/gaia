use crate::pb::sf::substreams::rpc::v2::BlockScopedData;
use chrono::DateTime;

pub fn output(block_data: &BlockScopedData) -> &prost_types::Any {
    return block_data
        .output
        .as_ref()
        .unwrap()
        .map_output
        .as_ref()
        .unwrap();
}

#[derive(Clone)]
pub struct BlockMetadata {
    pub cursor: String,
    pub block_number: u64,
    pub timestamp: String,
}

pub fn block_metadata(block_data: &BlockScopedData) -> BlockMetadata {
    let clock = block_data.clock.as_ref().unwrap();
    let timestamp = clock.timestamp.as_ref().unwrap();
    let date = DateTime::from_timestamp(timestamp.seconds, timestamp.nanos as u32)
        .expect("received timestamp should always be valid");

    return BlockMetadata {
        timestamp: (date
            .signed_duration_since(chrono::offset::Utc::now())
            .num_seconds()
            * -1)
            .to_string(),
        block_number: clock.number,
        cursor: block_data.cursor.clone(),
    };
}
