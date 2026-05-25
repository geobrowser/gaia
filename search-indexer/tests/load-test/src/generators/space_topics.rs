use anyhow::Result;
use prost::Message;
use uuid::Uuid;

use hermes_schema::pb::topics::{HermesTopicDeclared, HermesTopicRemoved};

/// Generate a HermesTopicDeclared protobuf message for the space.topics Kafka topic.
pub fn create_topic_declared(space_id: Uuid, topic_id: Uuid) -> Result<Vec<u8>> {
    let msg = HermesTopicDeclared {
        space_id: space_id.as_bytes().to_vec(),
        topic_id: topic_id.as_bytes().to_vec(),
        meta: None,
    };

    let mut buf = Vec::new();
    msg.encode(&mut buf)?;
    Ok(buf)
}

/// Generate a HermesTopicRemoved protobuf message for the space.topics Kafka topic.
///
/// Paired with the `event-type: TOPIC_REMOVED` Kafka header.
pub fn create_topic_removed(space_id: Uuid, topic_id: Uuid) -> Result<Vec<u8>> {
    let msg = HermesTopicRemoved {
        space_id: space_id.as_bytes().to_vec(),
        topic_id: topic_id.as_bytes().to_vec(),
        meta: None,
    };

    let mut buf = Vec::new();
    msg.encode(&mut buf)?;
    Ok(buf)
}
