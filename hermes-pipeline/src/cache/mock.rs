//! Mock IPFS cache for development/testing.
//!
//! Pre-populated with GRC-20 v2 payload bytes for testing.
//!
//! Note: This is a simplified mock for v2 migration testing.
//! It uses grc-20 crate to encode test edits into GRC2 format.

use std::borrow::Cow;
use std::collections::HashMap;

use async_trait::async_trait;
use grc_20::genesis::properties;
use grc_20::{CreateEntity, Edit, Op, PropertyValue, Value as Grc20Value, encode_edit};

use super::{CacheError, CachedEdit, IpfsCache};

// =============================================================================
// Well-known IDs for mock edits
// =============================================================================

const fn make_id(last_byte: u8) -> [u8; 16] {
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, last_byte]
}

// Entity IDs
const ENTITY_PERSON_1: [u8; 16] = make_id(0xF1);
const ENTITY_PERSON_2: [u8; 16] = make_id(0xF2);
const ENTITY_ORG_1: [u8; 16] = make_id(0xF3);
const ENTITY_PROJECT_1: [u8; 16] = make_id(0xF4);
const ENTITY_DOC_1: [u8; 16] = make_id(0xF5);
const ENTITY_TOPIC_1: [u8; 16] = make_id(0xF6);

// Type Entity IDs
const TYPE_PERSON: [u8; 16] = make_id(0xB1);
const TYPE_ORGANIZATION: [u8; 16] = make_id(0xB2);
const TYPE_PROJECT: [u8; 16] = make_id(0xB3);

// Edit IDs
const EDIT_ROOT_1: [u8; 16] = make_id(0xE1);
const EDIT_ROOT_2: [u8; 16] = make_id(0xE2);
const EDIT_ROOT_3: [u8; 16] = make_id(0xE3);
const EDIT_ROOT_4: [u8; 16] = make_id(0xE4);
const EDIT_ROOT_5: [u8; 16] = make_id(0xE5);
const EDIT_A_1: [u8; 16] = make_id(0xEA);
const EDIT_A_2: [u8; 16] = make_id(0xEB);
const EDIT_B_1: [u8; 16] = make_id(0xEC);
const EDIT_C_1: [u8; 16] = make_id(0xED);

// Author ID (16 bytes for grc-20)
const AUTHOR_1: [u8; 16] = make_id(0x11);

/// Mock IPFS cache with pre-populated GRC-20 v2 payload bytes.
pub struct MockIpfsCache {
    payloads: HashMap<String, Vec<u8>>,
    errored_hashes: std::collections::HashSet<String>,
}

impl MockIpfsCache {
    pub fn new() -> Self {
        let mut cache = Self {
            payloads: HashMap::new(),
            errored_hashes: std::collections::HashSet::new(),
        };
        cache.populate();
        cache
    }

    #[allow(dead_code)]
    pub fn with_errored_hashes(errored: Vec<String>) -> Self {
        let mut cache = Self::new();
        cache.errored_hashes = errored.into_iter().collect();
        cache
    }

    fn populate(&mut self) {
        // Root space edits
        self.insert_edit("QmRootEdit1CreatePersons", create_persons_edit());
        self.insert_edit("QmRootEdit2AddDescriptions", create_descriptions_edit());
        self.insert_edit("QmRootEdit3CreateTypes", create_types_edit());
        self.insert_edit(
            "QmRootEdit4CreateTypeRelations",
            create_type_relations_edit(),
        );
        self.insert_edit("QmRootEdit5DeleteTypeRelation", delete_type_relation_edit());

        // Space edits
        self.insert_edit("QmSpaceAEdit1CreateOrg", create_org_edit());
        self.insert_edit("QmSpaceAEdit2CreateRelations", create_relations_edit());
        self.insert_edit("QmSpaceBEdit1CreateDoc", create_doc_edit());
        self.insert_edit("QmSpaceCEdit1CreateTopic", create_topic_edit());
    }

    fn insert_edit(&mut self, hash: &str, edit: Edit) {
        if let Ok(bytes) = encode_edit(&edit) {
            self.payloads.insert(hash.to_string(), bytes);
        }
    }
}

impl Default for MockIpfsCache {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl IpfsCache for MockIpfsCache {
    async fn get(&self, ipfs_hash: &str, space_id: &[u8]) -> Result<CachedEdit, CacheError> {
        if self.errored_hashes.contains(ipfs_hash) {
            return Ok(CachedEdit::errored(
                ipfs_hash.to_string(),
                space_id.to_vec(),
            ));
        }

        match self.payloads.get(ipfs_hash) {
            Some(payload) => Ok(CachedEdit::success(
                ipfs_hash.to_string(),
                payload.clone(),
                space_id.to_vec(),
            )),
            None => Err(CacheError::NotFound(ipfs_hash.to_string())),
        }
    }

    async fn get_batch(&self, requests: &[(&str, &[u8])]) -> Vec<Result<CachedEdit, CacheError>> {
        let mut results = Vec::with_capacity(requests.len());
        for (ipfs_hash, space_id) in requests {
            results.push(self.get(ipfs_hash, space_id).await);
        }
        results
    }
}

// =============================================================================
// Edit creation helpers
// =============================================================================

fn text_value(property: [u8; 16], text: &str) -> PropertyValue<'static> {
    PropertyValue {
        property,
        value: Grc20Value::Text {
            value: Cow::Owned(text.to_string()),
            language: None,
        },
    }
}

fn create_persons_edit() -> Edit<'static> {
    Edit {
        id: EDIT_ROOT_1,
        name: Cow::Borrowed("Create Persons"),
        authors: vec![AUTHOR_1],
        created_at: 1700000000,
        ops: vec![
            Op::CreateEntity(CreateEntity {
                id: ENTITY_PERSON_1,
                values: vec![text_value(properties::name(), "Alice")],
                context: None,
            }),
            Op::CreateEntity(CreateEntity {
                id: ENTITY_PERSON_2,
                values: vec![text_value(properties::name(), "Bob")],
                context: None,
            }),
        ],
    }
}

fn create_descriptions_edit() -> Edit<'static> {
    Edit {
        id: EDIT_ROOT_2,
        name: Cow::Borrowed("Add Descriptions"),
        authors: vec![AUTHOR_1],
        created_at: 1700000001,
        ops: vec![
            Op::CreateEntity(CreateEntity {
                id: ENTITY_PERSON_1,
                values: vec![text_value(
                    properties::description(),
                    "A software developer",
                )],
                context: None,
            }),
            Op::CreateEntity(CreateEntity {
                id: ENTITY_PERSON_2,
                values: vec![text_value(properties::description(), "A project manager")],
                context: None,
            }),
        ],
    }
}

fn create_types_edit() -> Edit<'static> {
    Edit {
        id: EDIT_ROOT_3,
        name: Cow::Borrowed("Create Types"),
        authors: vec![AUTHOR_1],
        created_at: 1700000002,
        ops: vec![
            Op::CreateEntity(CreateEntity {
                id: TYPE_PERSON,
                values: vec![text_value(properties::name(), "Person")],
                context: None,
            }),
            Op::CreateEntity(CreateEntity {
                id: TYPE_ORGANIZATION,
                values: vec![text_value(properties::name(), "Organization")],
                context: None,
            }),
            Op::CreateEntity(CreateEntity {
                id: TYPE_PROJECT,
                values: vec![text_value(properties::name(), "Project")],
                context: None,
            }),
        ],
    }
}

fn create_type_relations_edit() -> Edit<'static> {
    // Simplified: just creates entities, no relations for now
    Edit {
        id: EDIT_ROOT_4,
        name: Cow::Borrowed("Create Type Relations"),
        authors: vec![AUTHOR_1],
        created_at: 1700000003,
        ops: vec![],
    }
}

fn delete_type_relation_edit() -> Edit<'static> {
    Edit {
        id: EDIT_ROOT_5,
        name: Cow::Borrowed("Delete Type Relation"),
        authors: vec![AUTHOR_1],
        created_at: 1700000004,
        ops: vec![],
    }
}

fn create_org_edit() -> Edit<'static> {
    Edit {
        id: EDIT_A_1,
        name: Cow::Borrowed("Create Organization"),
        authors: vec![AUTHOR_1],
        created_at: 1700000005,
        ops: vec![Op::CreateEntity(CreateEntity {
            id: ENTITY_ORG_1,
            values: vec![
                text_value(properties::name(), "Acme Corp"),
                text_value(properties::description(), "A technology company"),
            ],
            context: None,
        })],
    }
}

fn create_relations_edit() -> Edit<'static> {
    // Simplified: empty for now
    Edit {
        id: EDIT_A_2,
        name: Cow::Borrowed("Create Relations"),
        authors: vec![AUTHOR_1],
        created_at: 1700000006,
        ops: vec![],
    }
}

fn create_doc_edit() -> Edit<'static> {
    Edit {
        id: EDIT_B_1,
        name: Cow::Borrowed("Create Document"),
        authors: vec![AUTHOR_1],
        created_at: 1700000007,
        ops: vec![
            Op::CreateEntity(CreateEntity {
                id: ENTITY_PROJECT_1,
                values: vec![text_value(properties::name(), "Project Alpha")],
                context: None,
            }),
            Op::CreateEntity(CreateEntity {
                id: ENTITY_DOC_1,
                values: vec![text_value(properties::name(), "Technical Specification")],
                context: None,
            }),
        ],
    }
}

fn create_topic_edit() -> Edit<'static> {
    Edit {
        id: EDIT_C_1,
        name: Cow::Borrowed("Create Topic"),
        authors: vec![AUTHOR_1],
        created_at: 1700000008,
        ops: vec![Op::CreateEntity(CreateEntity {
            id: ENTITY_TOPIC_1,
            values: vec![
                text_value(properties::name(), "Blockchain Technology"),
                text_value(properties::description(), "Distributed ledger technology"),
            ],
            context: None,
        })],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grc_20::decode_edit;

    #[tokio::test]
    async fn test_cache_has_all_mock_edits() {
        let cache = MockIpfsCache::new();
        let space_id = vec![0x01; 16];

        // All edits should be present
        assert!(
            cache
                .get("QmRootEdit1CreatePersons", &space_id)
                .await
                .is_ok()
        );
        assert!(
            cache
                .get("QmRootEdit2AddDescriptions", &space_id)
                .await
                .is_ok()
        );
        assert!(cache.get("QmRootEdit3CreateTypes", &space_id).await.is_ok());
        assert!(cache.get("QmSpaceAEdit1CreateOrg", &space_id).await.is_ok());
        assert!(cache.get("QmSpaceBEdit1CreateDoc", &space_id).await.is_ok());
        assert!(
            cache
                .get("QmSpaceCEdit1CreateTopic", &space_id)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn test_cache_miss_returns_not_found() {
        let cache = MockIpfsCache::new();
        let space_id = vec![0x01; 16];

        let result = cache.get("QmNonExistentHash", &space_id).await;
        assert!(matches!(result, Err(CacheError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_cached_payload_is_valid_grc20() {
        let cache = MockIpfsCache::new();
        let space_id = vec![0x01; 16];

        let result = cache
            .get("QmRootEdit1CreatePersons", &space_id)
            .await
            .unwrap();
        assert!(!result.is_errored);
        assert!(result.has_content());

        // Verify payload is valid GRC2 format
        let payload = result.payload.unwrap();
        let edit = decode_edit(&payload).expect("Should decode as valid GRC-20");
        assert_eq!(edit.name.as_ref(), "Create Persons");
        assert_eq!(edit.ops.len(), 2);
    }

    #[tokio::test]
    async fn test_errored_hash() {
        let cache = MockIpfsCache::with_errored_hashes(vec!["QmRootEdit1CreatePersons".into()]);
        let space_id = vec![0x01; 16];

        let result = cache
            .get("QmRootEdit1CreatePersons", &space_id)
            .await
            .unwrap();
        assert!(result.is_errored);
        assert!(!result.has_content());
        assert!(result.payload.is_none());
    }
}
