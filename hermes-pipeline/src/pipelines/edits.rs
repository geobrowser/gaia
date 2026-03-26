//! Pipeline: EDITS_PUBLISHED → knowledge.edits
//!
//! Converts edit published actions to HermesEdit events.
//! Uses prefetched IPFS cache data to resolve content.
//!
//! Note: As of v2, the cache returns raw GRC2/GRC2Z payload bytes.
//! We decode the header to populate HermesEdit fields for observability,
//! but the full payload is passed to kg-indexer for decoding.

use std::collections::HashMap;

use anyhow::Result;
use grc_20::decode_edit;
use hermes_instrumentation::warn;
use hermes_kafka::KAFKA_MESSAGE_MAX_BYTES;
use hermes_relay::{Action, actions, extract_ipfs_uri};
use hermes_schema::pb::knowledge::HermesEdit;
use prost::Message;

use crate::cache::CachedEdit;

use super::BlockMetadata;

/// Result of transforming edit actions.
#[derive(Debug, Default)]
pub struct TransformResult {
    /// Transformed edit events ready for emission.
    pub events: Vec<HermesEdit>,
    /// Number of cache misses.
    pub cache_misses: u64,
    /// Number of errored entries (cache marked them as failed).
    pub errored_entries: u64,
    /// Number of edits skipped because the encoded Kafka message would exceed the broker limit.
    pub oversized_events: u64,
}

/// Transform all EDITS_PUBLISHED actions in a block using prefetched cache data.
///
/// This function:
/// 1. Filters actions for EDITS_PUBLISHED
/// 2. Looks up edit content from the prefetched cache
/// 3. Converts successful lookups to HermesEdit events
pub fn transform(
    actions: &[Action],
    meta: &BlockMetadata,
    prefetched: &HashMap<String, CachedEdit>,
) -> Result<TransformResult> {
    let mut result = TransformResult::default();

    for (index, action) in actions.iter().enumerate() {
        if !actions::matches(&action.action, &actions::EDITS_PUBLISHED) {
            continue;
        }

        // Extract IPFS URI from action data
        let ipfs_uri = match extract_ipfs_uri(&action.data) {
            Some(uri) => uri,
            None => {
                let space_id = hex::encode(&action.from_id);
                let data_prefix_len = action.data.len().min(64);
                let data_prefix = hex::encode(&action.data[..data_prefix_len]);
                warn!(
                    block = meta.block_number,
                    space_id = %space_id,
                    data_len = action.data.len(),
                    data_prefix = %data_prefix,
                    "EDITS_PUBLISHED missing valid IPFS URI, skipping"
                );
                continue;
            }
        };

        // Look up from prefetched cache
        match prefetched.get(&ipfs_uri) {
            Some(cached_edit) => {
                if cached_edit.is_errored {
                    result.errored_entries += 1;
                } else if let Some(payload) = &cached_edit.payload {
                    match convert(action, payload, meta, index as u32) {
                        Ok(event) => {
                            if let Err(encoded_len) =
                                validate_kafka_message_size(&event, KAFKA_MESSAGE_MAX_BYTES)
                            {
                                warn!(
                                    ipfs_uri = %ipfs_uri,
                                    space_id = %hex::encode(&action.from_id),
                                    payload_bytes = payload.len(),
                                    encoded_message_bytes = encoded_len,
                                    kafka_limit_bytes = KAFKA_MESSAGE_MAX_BYTES,
                                    "EDITS_PUBLISHED exceeds Kafka max message size, skipping"
                                );
                                result.oversized_events += 1;
                                continue;
                            }

                            result.events.push(event);
                        }
                        Err(e) => {
                            warn!(
                                ipfs_uri = %ipfs_uri,
                                error = %e,
                                "Failed to convert edit payload"
                            );
                        }
                    }
                } else {
                    result.errored_entries += 1;
                }
            }
            None => {
                // Cache miss - this shouldn't happen if prefetch worked correctly
                warn!(
                    ipfs_uri = %ipfs_uri,
                    "Edit not found in prefetched cache"
                );
                result.cache_misses += 1;
            }
        }
    }

    Ok(result)
}

/// Convert an EDITS_PUBLISHED action with cached payload to HermesEdit proto.
///
/// The action structure for EDITS_PUBLISHED:
/// - from_id: space_id (16 bytes) - the space publishing the edit
/// - to_id: unused (zeros)
/// - topic: unused (zeros)
/// - data: IPFS hash as bytes
///
/// Note: We decode the GRC2/GRC2Z payload to extract header fields (id, name,
/// authors) for observability, but the full payload bytes are passed through
/// to kg-indexer for decoding.
fn convert(
    action: &Action,
    payload: &[u8],
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesEdit> {
    // Decode the edit to extract header fields
    // This is validated by hermes-ipfs-cache, so decode should succeed
    let edit = decode_edit(payload)
        .map_err(|e| anyhow::anyhow!("Failed to decode GRC-20 payload: {}", e))?;

    Ok(HermesEdit {
        id: edit.id.to_vec(),
        name: edit.name.to_string(),
        payload: payload.to_vec(),
        authors: edit.authors.iter().map(|a| a.to_vec()).collect(),
        language: None, // v2 doesn't have a language field at edit level
        space_id: action.from_id.clone(),
        is_canonical: true, // TODO: Determine from topology
        meta: Some(meta.to_proto(sequence)),
    })
}

fn validate_kafka_message_size<T: Message>(message: &T, max_bytes: usize) -> Result<(), usize> {
    let encoded_len = message.encoded_len();
    if encoded_len > max_bytes {
        Err(encoded_len)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipelines::prefetch::RetryConfig;
    use grc_20::genesis::properties;
    use grc_20::{
        CreateEntity, Edit as Grc20Edit, Op, PropertyValue, Value as Grc20Value, encode_edit,
    };
    use std::borrow::Cow;
    use std::time::Duration;

    fn test_meta() -> BlockMetadata {
        BlockMetadata {
            cursor: "test_cursor".to_string(),
            block_number: 12345,
            timestamp: "1234567890".to_string(),
        }
    }

    fn test_payload() -> Vec<u8> {
        let edit = Grc20Edit {
            id: [1u8; 16],
            name: Cow::Borrowed("Test Edit"),
            authors: vec![[4u8; 16]],
            created_at: 1700000000,
            ops: vec![Op::CreateEntity(CreateEntity {
                id: [2u8; 16],
                values: vec![PropertyValue {
                    property: properties::name(),
                    value: Grc20Value::Text {
                        value: Cow::Borrowed("test"),
                        language: None,
                    },
                }],
                context: None,
            })],
        };
        encode_edit(&edit).expect("Should encode test edit")
    }

    #[test]
    fn test_convert_payload() {
        let action = Action {
            from_id: vec![0x01; 16],
            to_id: vec![0; 16],
            action: actions::EDITS_PUBLISHED.to_vec(),
            topic: vec![0; 32],
            data: b"ipfs://QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG".to_vec(),
        };

        let payload = test_payload();
        let result = convert(&action, &payload, &test_meta(), 0).unwrap();

        assert_eq!(result.id, vec![1u8; 16]);
        assert_eq!(result.name, "Test Edit");
        assert!(!result.payload.is_empty());
        assert_eq!(result.space_id, vec![0x01; 16]);
        assert!(result.is_canonical);
    }

    #[test]
    fn test_validate_kafka_message_size_uses_encoded_len() {
        let event = HermesEdit {
            id: vec![1u8; 16],
            name: "Test Edit".into(),
            payload: vec![7u8; 32],
            authors: vec![vec![4u8; 16]],
            language: None,
            space_id: vec![0x01; 16],
            is_canonical: true,
            meta: Some(test_meta().to_proto(0)),
        };

        let encoded_len = event.encoded_len();

        assert_eq!(validate_kafka_message_size(&event, encoded_len), Ok(()));
        assert_eq!(
            validate_kafka_message_size(&event, encoded_len.saturating_sub(1)),
            Err(encoded_len)
        );
    }

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.initial_delay_ms, 10);
        assert_eq!(config.factor, 2);
        assert_eq!(config.max_delay, Duration::from_secs(5));
        assert_eq!(config.max_retries, 10);
    }
}
