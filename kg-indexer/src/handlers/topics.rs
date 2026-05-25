use hermes_schema::pb::topics::{HermesTopicDeclared, HermesTopicRemoved};
use uuid::Uuid;

use crate::error::HandlerError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpaceTopicAssignment {
    pub space_id: Uuid,
    pub topic_id: Uuid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpaceTopicRemoval {
    pub space_id: Uuid,
    pub topic_id: Uuid,
}

/// Process a HermesTopicDeclared message and return the space/topic assignment.
pub fn handle_topic_declared(
    event: &HermesTopicDeclared,
) -> Result<SpaceTopicAssignment, HandlerError> {
    let space_id = Uuid::from_slice(&event.space_id)?;
    let topic_id = Uuid::from_slice(&event.topic_id)?;

    Ok(SpaceTopicAssignment { space_id, topic_id })
}

/// Process a HermesTopicRemoved message and return the space/topic removal.
pub fn handle_topic_removed(event: &HermesTopicRemoved) -> Result<SpaceTopicRemoval, HandlerError> {
    let space_id = Uuid::from_slice(&event.space_id)?;
    let topic_id = Uuid::from_slice(&event.topic_id)?;

    Ok(SpaceTopicRemoval { space_id, topic_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_uuid_bytes(last_byte: u8) -> Vec<u8> {
        let mut bytes = vec![0u8; 15];
        bytes.push(last_byte);
        bytes
    }

    #[test]
    fn test_handle_topic_declared() {
        let event = HermesTopicDeclared {
            space_id: make_uuid_bytes(0x01),
            topic_id: make_uuid_bytes(0x02),
            meta: None,
        };

        let result = handle_topic_declared(&event).unwrap();

        assert_eq!(
            result.space_id,
            Uuid::from_slice(&make_uuid_bytes(0x01)).unwrap()
        );
        assert_eq!(
            result.topic_id,
            Uuid::from_slice(&make_uuid_bytes(0x02)).unwrap()
        );
    }

    #[test]
    fn test_handle_topic_declared_invalid_topic_id() {
        let event = HermesTopicDeclared {
            space_id: make_uuid_bytes(0x01),
            topic_id: vec![0u8; 15],
            meta: None,
        };

        let err = handle_topic_declared(&event).unwrap_err();
        assert!(matches!(err, HandlerError::InvalidUuidBytes(_)));
    }

    #[test]
    fn test_handle_topic_removed() {
        let event = HermesTopicRemoved {
            space_id: make_uuid_bytes(0x01),
            topic_id: make_uuid_bytes(0x02),
            meta: None,
        };

        let result = handle_topic_removed(&event).unwrap();

        assert_eq!(
            result.space_id,
            Uuid::from_slice(&make_uuid_bytes(0x01)).unwrap()
        );
        assert_eq!(
            result.topic_id,
            Uuid::from_slice(&make_uuid_bytes(0x02)).unwrap()
        );
    }

    #[test]
    fn test_handle_topic_removed_invalid_topic_id() {
        let event = HermesTopicRemoved {
            space_id: make_uuid_bytes(0x01),
            topic_id: vec![0u8; 15],
            meta: None,
        };

        let err = handle_topic_removed(&event).unwrap_err();
        assert!(matches!(err, HandlerError::InvalidUuidBytes(_)));
    }
}
