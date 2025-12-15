//! Mock IPFS cache for development/testing.
//!
//! Pre-populated with edit content matching the IPFS hashes emitted by
//! hermes-relay's mock test topology (see `hermes-relay/src/source/mock_events.rs`).

use std::collections::HashMap;

use async_trait::async_trait;
use wire::pb::grc20::{Edit, Entity, Op, Relation, Value};

use super::{CacheError, CachedEdit, IpfsCache};

// =============================================================================
// Well-known IDs for mock edits (matching hermes-relay test topology pattern)
// =============================================================================

// Entity IDs
const ENTITY_PERSON_1: [u8; 16] = make_id(0xF1);
const ENTITY_PERSON_2: [u8; 16] = make_id(0xF2);
const ENTITY_ORG_1: [u8; 16] = make_id(0xF3);
const ENTITY_PROJECT_1: [u8; 16] = make_id(0xF4);
const ENTITY_DOC_1: [u8; 16] = make_id(0xF5);
const ENTITY_TOPIC_1: [u8; 16] = make_id(0xF6);

// Property IDs
const PROPERTY_NAME: [u8; 16] = make_id(0xD1);
const PROPERTY_DESCRIPTION: [u8; 16] = make_id(0xD2);
#[allow(dead_code)]
const PROPERTY_URL: [u8; 16] = make_id(0xD3);

// Relation Type IDs
const RELATION_TYPE_BELONGS_TO: [u8; 16] = make_id(0xC2);
#[allow(dead_code)]
const RELATION_TYPE_RELATED_TO: [u8; 16] = make_id(0xC3);

// Edit IDs
const EDIT_ROOT_1: [u8; 16] = make_id(0xE1);
const EDIT_ROOT_2: [u8; 16] = make_id(0xE2);
const EDIT_A_1: [u8; 16] = make_id(0xEA);
const EDIT_A_2: [u8; 16] = make_id(0xEB);
const EDIT_B_1: [u8; 16] = make_id(0xEC);
const EDIT_C_1: [u8; 16] = make_id(0xED);

// Space IDs (for cross-space relations)
const SPACE_A: [u8; 16] = make_id(0x0A);
#[allow(dead_code)]
const SPACE_B: [u8; 16] = make_id(0x0B);
#[allow(dead_code)]
const SPACE_C: [u8; 16] = make_id(0x0C);

// Relation IDs
const RELATION_1: [u8; 16] = make_id(0xA1);
const RELATION_2: [u8; 16] = make_id(0xA2);

// Author addresses
const AUTHOR_1: [u8; 32] = make_address(0x11);
const AUTHOR_2: [u8; 32] = make_address(0x12);

/// Helper to create a well-known ID from a single byte.
const fn make_id(last_byte: u8) -> [u8; 16] {
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, last_byte]
}

/// Helper to create a well-known address from a single byte.
const fn make_address(last_byte: u8) -> [u8; 32] {
    [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, last_byte,
    ]
}

/// Mock IPFS cache with pre-populated test edits.
///
/// This cache simulates the behavior of a real IPFS cache:
/// - Returns `CachedEdit` for known hashes
/// - Returns `CacheError::NotFound` for unknown hashes
/// - Can simulate errored entries for testing
pub struct MockIpfsCache {
    edits: HashMap<String, Edit>,
    /// Set of IPFS hashes that should return as errored entries
    errored_hashes: std::collections::HashSet<String>,
}

impl MockIpfsCache {
    /// Create a new mock cache with pre-populated test edits.
    pub fn new() -> Self {
        let mut cache = Self {
            edits: HashMap::new(),
            errored_hashes: std::collections::HashSet::new(),
        };
        cache.populate();
        cache
    }

    /// Create a mock cache with specific errored hashes for testing.
    #[allow(dead_code)]
    pub fn with_errored_hashes(errored: Vec<String>) -> Self {
        let mut cache = Self::new();
        cache.errored_hashes = errored.into_iter().collect();
        cache
    }

    /// Populate the cache with test edits matching hermes-relay's mock events.
    fn populate(&mut self) {
        // Root space edits
        self.edits
            .insert("QmRootEdit1CreatePersons".into(), create_persons_edit());
        self.edits.insert(
            "QmRootEdit2AddDescriptions".into(),
            create_descriptions_edit(),
        );

        // Space A edits
        self.edits
            .insert("QmSpaceAEdit1CreateOrg".into(), create_org_edit());
        self.edits.insert(
            "QmSpaceAEdit2CreateRelations".into(),
            create_relations_edit(),
        );

        // Space B edits
        self.edits
            .insert("QmSpaceBEdit1CreateDoc".into(), create_doc_edit());

        // Space C edits
        self.edits
            .insert("QmSpaceCEdit1CreateTopic".into(), create_topic_edit());
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
        // Check if this hash is marked as errored
        if self.errored_hashes.contains(ipfs_hash) {
            return Ok(CachedEdit::errored(
                ipfs_hash.to_string(),
                space_id.to_vec(),
            ));
        }

        // Look up the edit
        match self.edits.get(ipfs_hash) {
            Some(edit) => Ok(CachedEdit::success(
                ipfs_hash.to_string(),
                edit.clone(),
                space_id.to_vec(),
            )),
            None => Err(CacheError::NotFound(ipfs_hash.to_string())),
        }
    }

    async fn get_batch(&self, requests: &[(&str, &[u8])]) -> Vec<Result<CachedEdit, CacheError>> {
        // Mock cache can just do sequential lookups since it's in-memory
        let mut results = Vec::with_capacity(requests.len());
        for (ipfs_hash, space_id) in requests {
            results.push(self.get(ipfs_hash, space_id).await);
        }
        results
    }
}

// =============================================================================
// Edit creation functions
// =============================================================================

/// Creates two person entities with names.
///
/// Edit for: QmRootEdit1CreatePersons (root space)
fn create_persons_edit() -> Edit {
    Edit {
        id: EDIT_ROOT_1.to_vec(),
        name: "Create Persons".into(),
        ops: vec![
            // Create Person 1 with name
            Op {
                payload: Some(wire::pb::grc20::op::Payload::UpdateEntity(Entity {
                    id: ENTITY_PERSON_1.to_vec(),
                    values: vec![Value {
                        property: PROPERTY_NAME.to_vec(),
                        value: "Alice".into(),
                        options: None,
                    }],
                })),
            },
            // Create Person 2 with name
            Op {
                payload: Some(wire::pb::grc20::op::Payload::UpdateEntity(Entity {
                    id: ENTITY_PERSON_2.to_vec(),
                    values: vec![Value {
                        property: PROPERTY_NAME.to_vec(),
                        value: "Bob".into(),
                        options: None,
                    }],
                })),
            },
        ],
        authors: vec![AUTHOR_1.to_vec()],
        language: None,
    }
}

/// Adds descriptions to the person entities.
///
/// Edit for: QmRootEdit2AddDescriptions (root space)
fn create_descriptions_edit() -> Edit {
    Edit {
        id: EDIT_ROOT_2.to_vec(),
        name: "Add Descriptions".into(),
        ops: vec![
            // Add description to Person 1
            Op {
                payload: Some(wire::pb::grc20::op::Payload::UpdateEntity(Entity {
                    id: ENTITY_PERSON_1.to_vec(),
                    values: vec![Value {
                        property: PROPERTY_DESCRIPTION.to_vec(),
                        value: "A software developer".into(),
                        options: None,
                    }],
                })),
            },
            // Add description to Person 2
            Op {
                payload: Some(wire::pb::grc20::op::Payload::UpdateEntity(Entity {
                    id: ENTITY_PERSON_2.to_vec(),
                    values: vec![Value {
                        property: PROPERTY_DESCRIPTION.to_vec(),
                        value: "A project manager".into(),
                        options: None,
                    }],
                })),
            },
        ],
        authors: vec![AUTHOR_1.to_vec()],
        language: None,
    }
}

/// Creates an organization entity.
///
/// Edit for: QmSpaceAEdit1CreateOrg (space A)
fn create_org_edit() -> Edit {
    Edit {
        id: EDIT_A_1.to_vec(),
        name: "Create Organization".into(),
        ops: vec![Op {
            payload: Some(wire::pb::grc20::op::Payload::UpdateEntity(Entity {
                id: ENTITY_ORG_1.to_vec(),
                values: vec![
                    Value {
                        property: PROPERTY_NAME.to_vec(),
                        value: "Acme Corp".into(),
                        options: None,
                    },
                    Value {
                        property: PROPERTY_DESCRIPTION.to_vec(),
                        value: "A technology company".into(),
                        options: None,
                    },
                ],
            })),
        }],
        authors: vec![AUTHOR_2.to_vec()],
        language: None,
    }
}

/// Creates relations linking persons to organization.
///
/// Edit for: QmSpaceAEdit2CreateRelations (space A)
fn create_relations_edit() -> Edit {
    Edit {
        id: EDIT_A_2.to_vec(),
        name: "Create Relations".into(),
        ops: vec![
            // Person 1 belongs to Org (cross-space relation)
            Op {
                payload: Some(wire::pb::grc20::op::Payload::CreateRelation(Relation {
                    id: RELATION_1.to_vec(),
                    r#type: RELATION_TYPE_BELONGS_TO.to_vec(),
                    from_entity: ENTITY_PERSON_1.to_vec(),
                    from_space: None, // Root space (implicit)
                    from_version: None,
                    to_entity: ENTITY_ORG_1.to_vec(),
                    to_space: Some(SPACE_A.to_vec()),
                    to_version: None,
                    entity: ENTITY_PERSON_1.to_vec(),
                    position: Some("0".into()),
                    verified: Some(true),
                })),
            },
            // Person 2 belongs to Org
            Op {
                payload: Some(wire::pb::grc20::op::Payload::CreateRelation(Relation {
                    id: RELATION_2.to_vec(),
                    r#type: RELATION_TYPE_BELONGS_TO.to_vec(),
                    from_entity: ENTITY_PERSON_2.to_vec(),
                    from_space: None, // Root space (implicit)
                    from_version: None,
                    to_entity: ENTITY_ORG_1.to_vec(),
                    to_space: Some(SPACE_A.to_vec()),
                    to_version: None,
                    entity: ENTITY_PERSON_2.to_vec(),
                    position: Some("1".into()),
                    verified: Some(true),
                })),
            },
        ],
        authors: vec![AUTHOR_2.to_vec()],
        language: None,
    }
}

/// Creates a document entity with project information.
///
/// Edit for: QmSpaceBEdit1CreateDoc (space B)
fn create_doc_edit() -> Edit {
    Edit {
        id: EDIT_B_1.to_vec(),
        name: "Create Document".into(),
        ops: vec![
            // Create project entity
            Op {
                payload: Some(wire::pb::grc20::op::Payload::UpdateEntity(Entity {
                    id: ENTITY_PROJECT_1.to_vec(),
                    values: vec![
                        Value {
                            property: PROPERTY_NAME.to_vec(),
                            value: "Project Alpha".into(),
                            options: None,
                        },
                        Value {
                            property: PROPERTY_DESCRIPTION.to_vec(),
                            value: "A groundbreaking project".into(),
                            options: None,
                        },
                    ],
                })),
            },
            // Create doc entity
            Op {
                payload: Some(wire::pb::grc20::op::Payload::UpdateEntity(Entity {
                    id: ENTITY_DOC_1.to_vec(),
                    values: vec![Value {
                        property: PROPERTY_NAME.to_vec(),
                        value: "Technical Specification".into(),
                        options: None,
                    }],
                })),
            },
        ],
        authors: vec![AUTHOR_1.to_vec(), AUTHOR_2.to_vec()],
        language: None,
    }
}

/// Creates a topic entity.
///
/// Edit for: QmSpaceCEdit1CreateTopic (space C)
fn create_topic_edit() -> Edit {
    Edit {
        id: EDIT_C_1.to_vec(),
        name: "Create Topic".into(),
        ops: vec![Op {
            payload: Some(wire::pb::grc20::op::Payload::UpdateEntity(Entity {
                id: ENTITY_TOPIC_1.to_vec(),
                values: vec![
                    Value {
                        property: PROPERTY_NAME.to_vec(),
                        value: "Blockchain Technology".into(),
                        options: None,
                    },
                    Value {
                        property: PROPERTY_DESCRIPTION.to_vec(),
                        value: "Distributed ledger technology".into(),
                        options: None,
                    },
                ],
            })),
        }],
        authors: vec![AUTHOR_1.to_vec()],
        language: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_has_all_mock_edits() {
        let cache = MockIpfsCache::new();
        let space_id = vec![0x01; 16];

        // All 6 edits from mock_events.rs should be present
        assert!(cache
            .get("QmRootEdit1CreatePersons", &space_id)
            .await
            .is_ok());
        assert!(cache
            .get("QmRootEdit2AddDescriptions", &space_id)
            .await
            .is_ok());
        assert!(cache.get("QmSpaceAEdit1CreateOrg", &space_id).await.is_ok());
        assert!(cache
            .get("QmSpaceAEdit2CreateRelations", &space_id)
            .await
            .is_ok());
        assert!(cache.get("QmSpaceBEdit1CreateDoc", &space_id).await.is_ok());
        assert!(cache
            .get("QmSpaceCEdit1CreateTopic", &space_id)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_cache_miss_returns_not_found() {
        let cache = MockIpfsCache::new();
        let space_id = vec![0x01; 16];

        let result = cache.get("QmNonExistentHash", &space_id).await;
        assert!(matches!(result, Err(CacheError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_cached_edit_structure() {
        let cache = MockIpfsCache::new();
        let space_id = vec![0x01; 16];

        let result = cache
            .get("QmRootEdit1CreatePersons", &space_id)
            .await
            .unwrap();

        assert_eq!(result.cid, "QmRootEdit1CreatePersons");
        assert!(!result.is_errored);
        assert!(result.has_content());

        let edit = result.edit.unwrap();
        assert_eq!(edit.name, "Create Persons");
        assert_eq!(edit.ops.len(), 2);
        assert_eq!(edit.authors.len(), 1);
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
        assert!(result.edit.is_none());
    }

    #[tokio::test]
    async fn test_batch_get() {
        let cache = MockIpfsCache::new();
        let space_id = vec![0x01; 16];

        let requests: Vec<(&str, &[u8])> = vec![
            ("QmRootEdit1CreatePersons", &space_id),
            ("QmNonExistentHash", &space_id),
            ("QmSpaceAEdit1CreateOrg", &space_id),
        ];

        let results = cache.get_batch(&requests).await;

        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok());
        assert!(matches!(results[1], Err(CacheError::NotFound(_))));
        assert!(results[2].is_ok());
    }
}
