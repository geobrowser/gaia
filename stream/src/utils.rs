use crate::pb::sf::substreams::rpc::v2::BlockScopedData;
use chrono::{DateTime, Utc};

pub fn output(block_data: &BlockScopedData) -> &prost_types::Any {
    block_data
        .output
        .as_ref()
        .unwrap()
        .map_output
        .as_ref()
        .unwrap()
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

    BlockMetadata {
        timestamp: timestamp.seconds.to_string(),
        block_number: clock.number,
        cursor: block_data.cursor.clone(),
    }
}

pub fn format_drift(block_metadata: &BlockMetadata) -> String {
    let now = Utc::now();
    let block_timestamp_seconds: i64 = block_metadata.timestamp.parse().unwrap_or(0);
    let block_datetime =
        DateTime::from_timestamp(block_timestamp_seconds, 0).unwrap_or_else(Utc::now);

    let drift = now - block_datetime;
    let days = drift.num_days();
    let hours = drift.num_hours() % 24;
    let minutes = drift.num_minutes() % 60;
    let seconds = drift.num_seconds() % 60;
    let milliseconds = drift.num_milliseconds() % 1000;

    if days > 0 {
        format!(
            "{}d {}h {}m {}s {}ms",
            days, hours, minutes, seconds, milliseconds
        )
    } else if hours > 0 {
        format!("{}h {}m {}s {}ms", hours, minutes, seconds, milliseconds)
    } else if minutes > 0 {
        format!("{}m {}s {}ms", minutes, seconds, milliseconds)
    } else if seconds > 0 {
        format!("{}s {}ms", seconds, milliseconds)
    } else {
        format!("{}ms", milliseconds)
    }
}
