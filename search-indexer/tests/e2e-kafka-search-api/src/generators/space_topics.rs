use anyhow::Result;
use prost::Message;
use uuid::Uuid;

use hermes_schema::pb::topics::{HermesTopicDeclared, HermesTopicRemoved};

/// Generate a HermesTopicDeclared protobuf message for the space.topics Kafka topic.
///
/// This creates a message declaring that a space has a topic entity, which the
/// search-indexer uses to enrich search results with space metadata (name,
/// description, avatar, cover) by looking up the topic entity in the index.
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
/// Used together with the `event-type: TOPIC_REMOVED` Kafka header so the
/// search-indexer routes the payload through the removal path: cache cleared
/// and `space_topic_entity_id` removed from all docs in the space.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_topic_declared() {
        let space_id = Uuid::new_v4();
        let topic_id = Uuid::new_v4();

        let result = create_topic_declared(space_id, topic_id);
        assert!(result.is_ok());

        let bytes = result.unwrap();
        assert!(!bytes.is_empty());

        // Verify we can decode it
        let decoded = HermesTopicDeclared::decode(&bytes[..]);
        assert!(decoded.is_ok());

        let msg = decoded.unwrap();
        assert_eq!(msg.space_id, space_id.as_bytes().to_vec());
        assert_eq!(msg.topic_id, topic_id.as_bytes().to_vec());
        assert!(msg.meta.is_none());
    }

    #[test]
    fn test_create_topic_removed() {
        let space_id = Uuid::new_v4();
        let topic_id = Uuid::new_v4();

        let bytes = create_topic_removed(space_id, topic_id).unwrap();
        assert!(!bytes.is_empty());

        let decoded = HermesTopicRemoved::decode(&bytes[..]).unwrap();
        assert_eq!(decoded.space_id, space_id.as_bytes().to_vec());
        assert_eq!(decoded.topic_id, topic_id.as_bytes().to_vec());
        assert!(decoded.meta.is_none());
    }
}
