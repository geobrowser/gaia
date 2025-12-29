mod pb;
use hex_literal::hex;
use pb::actions::v1::{Action, Actions};
use pb::sf::ethereum::r#type::v2::{Block, Log};

substreams_ethereum::init!();

const FACTORY_TRACKED_CONTRACTS: &[[u8; 20]] = &[hex!("80eF8d87fafCB65F5399c6d28c72A27577616339")];

#[substreams::handlers::map]
fn map_actions(blk: Block) -> Result<Actions, substreams::errors::Error> {
    let mut actions = Actions::default();

    for transaction in &blk.transaction_traces {
        if let Some(receipt) = &transaction.receipt {
            let tx_hash = format!("0x{}", hex::encode(&transaction.hash));
            let block_number = blk.number;
            let block_timestamp = blk
                .header
                .as_ref()
                .map(|h| h.timestamp.as_ref().map(|t| t.seconds as u64))
                .flatten()
                .unwrap_or(0);

            for log in &receipt.logs {
                if is_address_in_contracts(&log.address) {
                    if let Some(action) = decode_action_log(&log)? {
                        let action = Action {
                            action_type: action.action_type,
                            action_version: action.action_version,
                            sender: format!("0x{}", hex::encode(&transaction.from)),
                            object_id: action.object_id,
                            group_id: action.group_id,
                            space_pov: action.space_pov,
                            metadata: action.metadata,
                            block_number,
                            block_timestamp,
                            tx_hash: tx_hash.clone(),
                            object_type: action.object_type,
                        };
                        actions.actions.push(action);
                    }
                }
            }
        }
    }

    Ok(actions)
}

fn is_address_in_contracts(address: &Vec<u8>) -> bool {
    if address.len() != 20 {
        return false;
    }

    FACTORY_TRACKED_CONTRACTS.contains(&address.as_slice().try_into().unwrap())
}

/// Decoded event data from the Action event log
struct EventData {
    action_type: u64,
    action_version: u64,
    object_type: u64,
    space_pov: String,
    group_id: Option<String>,
    object_id: String,
    metadata: Option<Vec<u8>>,
}

/// Decode an Action event log into an Action message
/// Based on the Action event signature from the smart contract
fn decode_action_log(log: &Log) -> Result<Option<EventData>, substreams::errors::Error> {
    let data = &log.data;
    if data.len() < 128 {
        return Ok(None);
    }

    let word0 = &data[0..32];
    let action_type = u16::from_be_bytes([word0[0], word0[1]]) as u64;
    let action_version = u16::from_be_bytes([word0[2], word0[3]]) as u64;
    let object_type = (word0[15] & 0x0F) as u64;
    let space_uuid_bytes = &word0[16..32];

    let space_pov = format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        space_uuid_bytes[0], space_uuid_bytes[1], space_uuid_bytes[2], space_uuid_bytes[3],
        space_uuid_bytes[4], space_uuid_bytes[5], space_uuid_bytes[6], space_uuid_bytes[7],
        space_uuid_bytes[8], space_uuid_bytes[9], space_uuid_bytes[10], space_uuid_bytes[11],
        space_uuid_bytes[12], space_uuid_bytes[13], space_uuid_bytes[14], space_uuid_bytes[15]
    );

    let word1 = &data[32..64];
    let group_id_bytes = &word1[0..16];
    let object_id_bytes = &word1[16..32];

    let object_id = format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        object_id_bytes[0], object_id_bytes[1], object_id_bytes[2], object_id_bytes[3],
        object_id_bytes[4], object_id_bytes[5], object_id_bytes[6], object_id_bytes[7],
        object_id_bytes[8], object_id_bytes[9], object_id_bytes[10], object_id_bytes[11],
        object_id_bytes[12], object_id_bytes[13], object_id_bytes[14], object_id_bytes[15]
    );

    let group_id = if group_id_bytes.iter().all(|&x| x == 0) {
        None
    } else {
        Some(format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            group_id_bytes[0], group_id_bytes[1], group_id_bytes[2], group_id_bytes[3],
            group_id_bytes[4], group_id_bytes[5], group_id_bytes[6], group_id_bytes[7],
            group_id_bytes[8], group_id_bytes[9], group_id_bytes[10], group_id_bytes[11],
            group_id_bytes[12], group_id_bytes[13], group_id_bytes[14], group_id_bytes[15]
        ))
    };

    let metadata = if data.len() < 128 {
        None
    } else {
        let payload_offset = u64::from_be_bytes([
            data[120], data[121], data[122], data[123], data[124], data[125], data[126], data[127],
        ]) as usize;

        if payload_offset + 32 <= data.len() {
            let payload_length = u64::from_be_bytes([
                data[payload_offset + 24],
                data[payload_offset + 25],
                data[payload_offset + 26],
                data[payload_offset + 27],
                data[payload_offset + 28],
                data[payload_offset + 29],
                data[payload_offset + 30],
                data[payload_offset + 31],
            ]) as usize;

            if payload_length > 0 && data.len() >= payload_offset + 32 + payload_length {
                Some(data[payload_offset + 32..payload_offset + 32 + payload_length].to_vec())
            } else {
                None
            }
        } else {
            None
        }
    };

    Ok(Some(EventData {
        action_type,
        action_version,
        object_type,
        object_id,
        group_id,
        space_pov,
        metadata,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pb::sf::ethereum::r#type::v2::Log;

    pub fn log_with_data(data: Vec<u8>) -> Log {
        Log {
            data,
            address: vec![0; 20],
            topics: vec![],
            index: 0,
            block_index: 0,
            ordinal: 0,
        }
    }

    const TEST_CASES: &[&str] = &[
        "00000001000000000000000000000000f8d9744df54645f1a90dcbd511c9d600
         00000000000000000000000000000000a8f03660921b4f2ab3c6e3cb9542748d
         00000000000000000000000049894dbfa9d78bbe4807da28efaca9d105e5244f
         0000000000000000000000000000000000000000000000000000000000000080
         0000000000000000000000000000000000000000000000000000000000000001
         0000000000000000000000000000000000000000000000000000000000000000", // Upvote on entity
        "00000001000000000000000000000001f8d9744df54645f1a90dcbd511c9d600
         00000000000000000000000000000000a8f03660921b4f2ab3c6e3cb9542748d
         00000000000000000000000049894dbfa9d78bbe4807da28efaca9d105e5244f
         0000000000000000000000000000000000000000000000000000000000000080
         0000000000000000000000000000000000000000000000000000000000000001
         0000000000000000000000000000000000000000000000000000000000000000", // Upvote on relation
        "00000001000000000000000000000000f8d9744df54645f1a90dcbd511c9d600
         00000000000000000000000000000000a8f03660921b4f2ab3c6e3cb9542749a
         00000000000000000000000049894dbfa9d78bbe4807da28efaca9d105e5244f
         0000000000000000000000000000000000000000000000000000000000000080
         0000000000000000000000000000000000000000000000000000000000000001
         0100000000000000000000000000000000000000000000000000000000000000", // Downvote on entity
        "00000001000000000000000000000001f8d9744df54645f1a90dcbd511c9d600
         00000000000000000000000000000000a8f03660921b4f2ab3c6e3cb9542749a
         00000000000000000000000049894dbfa9d78bbe4807da28efaca9d105e5244f
         0000000000000000000000000000000000000000000000000000000000000080
         0000000000000000000000000000000000000000000000000000000000000001
         0200000000000000000000000000000000000000000000000000000000000000", // Remove on relation
    ];

    #[test]
    fn test_decode_action_log() {
        let hex_string = TEST_CASES[0]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();
        let data = hex::decode(hex_string).unwrap();
        let log = log_with_data(data);
        let result = decode_action_log(&log);
        assert!(result.is_ok());
        let event_data = result.unwrap().unwrap();
        assert_eq!(event_data.action_version, 1);
        assert_eq!(event_data.action_type, 0);
        assert_eq!(event_data.object_type, 0);
        assert_eq!(event_data.space_pov, "f8d9744d-f546-45f1-a90d-cbd511c9d600");
        assert_eq!(event_data.group_id, None);
        assert_eq!(event_data.object_id, "a8f03660-921b-4f2a-b3c6-e3cb9542748d");
        assert_eq!(event_data.metadata, Some(vec![0]));
    }

    #[test]
    fn test_decode_action_log_relation() {
        let hex_string = TEST_CASES[1]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();
        let data = hex::decode(hex_string).unwrap();
        let log = log_with_data(data);
        let result = decode_action_log(&log);
        assert!(result.is_ok());
        let event_data = result.unwrap().unwrap();
        assert_eq!(event_data.action_version, 1);
        assert_eq!(event_data.action_type, 0);
        assert_eq!(event_data.object_type, 1);
        assert_eq!(event_data.space_pov, "f8d9744d-f546-45f1-a90d-cbd511c9d600");
        assert_eq!(event_data.group_id, None);
        assert_eq!(event_data.object_id, "a8f03660-921b-4f2a-b3c6-e3cb9542748d");
        assert_eq!(event_data.metadata, Some(vec![0]));
    }

    #[test]
    fn test_decode_action_log_downvote() {
        let hex_string = TEST_CASES[2]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();
        let data = hex::decode(hex_string).unwrap();
        let log = log_with_data(data);
        let result = decode_action_log(&log);
        assert!(result.is_ok());
        let event_data = result.unwrap().unwrap();
        assert_eq!(event_data.action_version, 1);
        assert_eq!(event_data.action_type, 0);
        assert_eq!(event_data.object_type, 0);
        assert_eq!(event_data.space_pov, "f8d9744d-f546-45f1-a90d-cbd511c9d600");
        assert_eq!(event_data.group_id, None);
        assert_eq!(event_data.object_id, "a8f03660-921b-4f2a-b3c6-e3cb9542749a");
        assert_eq!(event_data.metadata, Some(vec![1]));
    }

    #[test]
    fn test_decode_action_log_remove() {
        let hex_string = TEST_CASES[3]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();
        let data = hex::decode(hex_string).unwrap();
        let log = log_with_data(data);
        let result = decode_action_log(&log);
        assert!(result.is_ok());
        let event_data = result.unwrap().unwrap();
        assert_eq!(event_data.action_version, 1);
        assert_eq!(event_data.action_type, 0);
        assert_eq!(event_data.object_type, 1);
        assert_eq!(event_data.space_pov, "f8d9744d-f546-45f1-a90d-cbd511c9d600");
        assert_eq!(event_data.group_id, None);
        assert_eq!(event_data.object_id, "a8f03660-921b-4f2a-b3c6-e3cb9542749a");
        assert_eq!(event_data.metadata, Some(vec![2]));
    }
}
