//! Mock IPFS cache for development/testing.
//!
//! Pre-populated with edit content matching the IPFS hashes emitted by
//! hermes-relay's mock test topology (see `hermes-relay/src/source/mock_events.rs`).

use std::borrow::Cow;
use std::collections::HashMap;

use async_trait::async_trait;
use grc_20::{
    model::{Context, ContextEdge},
    CreateEntity, CreateRelation, DeleteRelation, Edit, Op, PropertyValue, UpdateEntity,
    UpdateRelation, UnsetRelationField, UnsetValue, Value as Grc20Value,
};

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
const ENTITY_SQUASH: [u8; 16] = make_id(0xF7);

// Property IDs
const PROPERTY_NAME: [u8; 16] = make_id(0xD1);
const PROPERTY_DESCRIPTION: [u8; 16] = make_id(0xD2);
#[allow(dead_code)]
const PROPERTY_URL: [u8; 16] = make_id(0xD3);

// Relation Type IDs
const RELATION_TYPE_BELONGS_TO: [u8; 16] = make_id(0xC2);
const RELATION_TYPE_RELATED_TO: [u8; 16] = make_id(0xC3);

// Edit IDs
const EDIT_ROOT_1: [u8; 16] = make_id(0xE1);
const EDIT_ROOT_2: [u8; 16] = make_id(0xE2);
const EDIT_ROOT_3: [u8; 16] = make_id(0xE3);
const EDIT_A_1: [u8; 16] = make_id(0xEA);
const EDIT_A_2: [u8; 16] = make_id(0xEB);
const EDIT_A_3: [u8; 16] = make_id(0xEE);
const EDIT_A_4: [u8; 16] = make_id(0xEF);
const EDIT_B_1: [u8; 16] = make_id(0xEC);
const EDIT_B_2: [u8; 16] = make_id(0xF0);
const EDIT_B_3: [u8; 16] = make_id(0xF8);
const EDIT_C_1: [u8; 16] = make_id(0xED);

// Space IDs (for cross-space relations)
const SPACE_A: [u8; 16] = make_id(0x0A);
const SPACE_B: [u8; 16] = make_id(0x0B);
#[allow(dead_code)]
const SPACE_C: [u8; 16] = make_id(0x0C);

// Relation IDs
const RELATION_1: [u8; 16] = make_id(0xA1);
const RELATION_2: [u8; 16] = make_id(0xA2);
const RELATION_3: [u8; 16] = make_id(0xA3);

// Author IDs
const AUTHOR_1: [u8; 16] = make_id(0x11);
const AUTHOR_2: [u8; 16] = make_id(0x12);

/// Helper to create a well-known ID from a single byte.
const fn make_id(last_byte: u8) -> [u8; 16] {
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, last_byte]
}

/// Helper to create a well-known address from a single byte.
#[allow(dead_code)]
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
    edits: HashMap<String, Vec<u8>>,
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
        self.edits
            .insert("QmRootEdit3DeleteName".into(), delete_person_2_name_edit());

        // Space A edits
        self.edits
            .insert("QmSpaceAEdit1CreateOrg".into(), create_org_edit());
        self.edits.insert(
            "QmSpaceAEdit2CreateRelations".into(),
            create_relations_edit(),
        );
        self.edits.insert(
            "QmSpaceAEdit3UpdateRelations".into(),
            update_relations_edit(),
        );
        self.edits.insert(
            "QmSpaceAEdit4UnsetRelationFields".into(),
            unset_relation_fields_edit(),
        );

        // Space B edits
        self.edits
            .insert("QmSpaceBEdit1CreateDoc".into(), create_doc_edit());
        self.edits
            .insert("QmSpaceBEdit2SquashOps".into(), squash_ops_edit());
        self.edits
            .insert("QmSpaceBEdit3DeleteRelation".into(), delete_relation_edit());

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
            Some(payload) => Ok(CachedEdit::success(
                ipfs_hash.to_string(),
                payload.clone(),
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
fn create_persons_edit() -> Vec<u8> {
    let edit = Edit {
        id: EDIT_ROOT_1,
        name: Cow::Borrowed("Create Persons"),
        authors: vec![AUTHOR_1],
        created_at: 1700000000,
        ops: vec![
            Op::CreateEntity(CreateEntity {
                id: ENTITY_PERSON_1,
                values: vec![PropertyValue {
                    property: PROPERTY_NAME,
                    value: Grc20Value::Text {
                        value: Cow::Borrowed("Alice"),
                        language: None,
                    },
                }],
                context: None,
            }),
            Op::CreateEntity(CreateEntity {
                id: ENTITY_PERSON_2,
                values: vec![PropertyValue {
                    property: PROPERTY_NAME,
                    value: Grc20Value::Text {
                        value: Cow::Borrowed("Bob"),
                        language: None,
                    },
                }],
                context: None,
            }),
        ],
    };

    grc_20::encode_edit(&edit).expect("Failed to encode edit")
}

/// Adds descriptions to the person entities.
///
/// Edit for: QmRootEdit2AddDescriptions (root space)
fn create_descriptions_edit() -> Vec<u8> {
    let edit = Edit {
        id: EDIT_ROOT_2,
        name: Cow::Borrowed("Add Descriptions"),
        authors: vec![AUTHOR_1],
        created_at: 1700000001,
        ops: vec![
            Op::UpdateEntity(UpdateEntity {
                id: ENTITY_PERSON_1,
                set_properties: vec![PropertyValue {
                    property: PROPERTY_DESCRIPTION,
                    value: Grc20Value::Text {
                        value: Cow::Borrowed("A software developer"),
                        language: None,
                    },
                }],
                unset_values: vec![],
                context: None,
            }),
            Op::UpdateEntity(UpdateEntity {
                id: ENTITY_PERSON_2,
                set_properties: vec![PropertyValue {
                    property: PROPERTY_DESCRIPTION,
                    value: Grc20Value::Text {
                        value: Cow::Borrowed("A project manager"),
                        language: None,
                    },
                }],
                unset_values: vec![],
                context: None,
            }),
        ],
    };

    grc_20::encode_edit(&edit).expect("Failed to encode edit")
}

/// Creates an organization entity.
///
/// Edit for: QmSpaceAEdit1CreateOrg (space A)
fn create_org_edit() -> Vec<u8> {
    let edit = Edit {
        id: EDIT_A_1,
        name: Cow::Borrowed("Create Organization"),
        authors: vec![AUTHOR_2],
        created_at: 1700000002,
        ops: vec![Op::CreateEntity(CreateEntity {
            id: ENTITY_ORG_1,
            values: vec![
                PropertyValue {
                    property: PROPERTY_NAME,
                    value: Grc20Value::Text {
                        value: Cow::Borrowed("Acme Corp"),
                        language: None,
                    },
                },
                PropertyValue {
                    property: PROPERTY_DESCRIPTION,
                    value: Grc20Value::Text {
                        value: Cow::Borrowed("A technology company"),
                        language: None,
                    },
                },
            ],
            context: Some(Context {
                root_id: ENTITY_ORG_1,
                edges: vec![ContextEdge {
                    type_id: RELATION_TYPE_BELONGS_TO,
                    to_entity_id: ENTITY_ORG_1,
                }],
            }),
        })],
    };

    grc_20::encode_edit(&edit).expect("Failed to encode edit")
}

/// Creates relations linking persons to organization.
///
/// Edit for: QmSpaceAEdit2CreateRelations (space A)
fn create_relations_edit() -> Vec<u8> {
    let edit = Edit {
        id: EDIT_A_2,
        name: Cow::Borrowed("Create Relations"),
        authors: vec![AUTHOR_2],
        created_at: 1700000003,
        ops: vec![
            Op::CreateRelation(CreateRelation {
                id: RELATION_1,
                entity: Some(ENTITY_PERSON_1),
                relation_type: RELATION_TYPE_BELONGS_TO,
                from: ENTITY_PERSON_1,
                from_is_value_ref: false,
                from_space: None,
                from_version: None,
                to: ENTITY_ORG_1,
                to_is_value_ref: false,
                to_space: Some(SPACE_A),
                to_version: None,
                position: Some(Cow::Borrowed("0")),
                context: Some(Context {
                    root_id: ENTITY_ORG_1,
                    edges: vec![ContextEdge {
                        type_id: RELATION_TYPE_BELONGS_TO,
                        to_entity_id: ENTITY_ORG_1,
                    }],
                }),
            }),
            Op::CreateRelation(CreateRelation {
                id: RELATION_2,
                entity: Some(ENTITY_PERSON_2),
                relation_type: RELATION_TYPE_BELONGS_TO,
                from: ENTITY_PERSON_2,
                from_is_value_ref: false,
                from_space: None,
                from_version: None,
                to: ENTITY_ORG_1,
                to_is_value_ref: false,
                to_space: Some(SPACE_A),
                to_version: None,
                position: Some(Cow::Borrowed("1")),
                context: Some(Context {
                    root_id: ENTITY_ORG_1,
                    edges: vec![ContextEdge {
                        type_id: RELATION_TYPE_BELONGS_TO,
                        to_entity_id: ENTITY_ORG_1,
                    }],
                }),
            }),
        ],
    };

    grc_20::encode_edit(&edit).expect("Failed to encode edit")
}

/// Creates a document entity with project information.
///
/// Edit for: QmSpaceBEdit1CreateDoc (space B)
fn create_doc_edit() -> Vec<u8> {
    let edit = Edit {
        id: EDIT_B_1,
        name: Cow::Borrowed("Create Document"),
        authors: vec![AUTHOR_1, AUTHOR_2],
        created_at: 1700000004,
        ops: vec![
            Op::CreateEntity(CreateEntity {
                id: ENTITY_PROJECT_1,
                values: vec![
                    PropertyValue {
                        property: PROPERTY_NAME,
                        value: Grc20Value::Text {
                            value: Cow::Borrowed("Project Alpha"),
                            language: None,
                        },
                    },
                    PropertyValue {
                        property: PROPERTY_DESCRIPTION,
                        value: Grc20Value::Text {
                            value: Cow::Borrowed("A groundbreaking project"),
                            language: None,
                        },
                    },
                ],
                context: None,
            }),
            Op::CreateEntity(CreateEntity {
                id: ENTITY_DOC_1,
                values: vec![PropertyValue {
                    property: PROPERTY_NAME,
                    value: Grc20Value::Text {
                        value: Cow::Borrowed("Technical Specification"),
                        language: None,
                    },
                }],
                context: None,
            }),
        ],
    };

    grc_20::encode_edit(&edit).expect("Failed to encode edit")
}

/// Creates a topic entity.
///
/// Edit for: QmSpaceCEdit1CreateTopic (space C)
fn create_topic_edit() -> Vec<u8> {
    let edit = Edit {
        id: EDIT_C_1,
        name: Cow::Borrowed("Create Topic"),
        authors: vec![AUTHOR_1],
        created_at: 1700000005,
        ops: vec![Op::CreateEntity(CreateEntity {
            id: ENTITY_TOPIC_1,
            values: vec![
                PropertyValue {
                    property: PROPERTY_NAME,
                    value: Grc20Value::Text {
                        value: Cow::Borrowed("Blockchain Technology"),
                        language: None,
                    },
                },
                PropertyValue {
                    property: PROPERTY_DESCRIPTION,
                    value: Grc20Value::Text {
                        value: Cow::Borrowed("Distributed ledger technology"),
                        language: None,
                    },
                },
            ],
            context: None,
        })],
    };

    grc_20::encode_edit(&edit).expect("Failed to encode edit")
}

/// Deletes the name value for Person 2.
///
/// Edit for: QmRootEdit3DeleteName (root space)
fn delete_person_2_name_edit() -> Vec<u8> {
    let edit = Edit {
        id: EDIT_ROOT_3,
        name: Cow::Borrowed("Delete Person 2 Name"),
        authors: vec![AUTHOR_1],
        created_at: 1700000006,
        ops: vec![Op::UpdateEntity(UpdateEntity {
            id: ENTITY_PERSON_2,
            set_properties: vec![],
            unset_values: vec![UnsetValue {
                property: PROPERTY_NAME,
                language: grc_20::UnsetLanguage::All,
            }],
            context: None,
        })],
    };

    grc_20::encode_edit(&edit).expect("Failed to encode edit")
}

/// Updates relation 1 with space + version metadata and new position.
///
/// Edit for: QmSpaceAEdit3UpdateRelations (space A)
fn update_relations_edit() -> Vec<u8> {
    let edit = Edit {
        id: EDIT_A_3,
        name: Cow::Borrowed("Update Relations"),
        authors: vec![AUTHOR_2],
        created_at: 1700000007,
        ops: vec![Op::UpdateRelation(UpdateRelation {
            id: RELATION_1,
            from_space: Some(SPACE_B),
            from_version: Some(EDIT_ROOT_2),
            to_space: Some(SPACE_A),
            to_version: Some(EDIT_A_1),
            position: Some(Cow::Borrowed("2")),
            unset: vec![],
            context: None,
        })],
    };

    grc_20::encode_edit(&edit).expect("Failed to encode edit")
}

/// Unsets relation metadata fields to verify squash/unset behavior.
///
/// Edit for: QmSpaceAEdit4UnsetRelationFields (space A)
fn unset_relation_fields_edit() -> Vec<u8> {
    let edit = Edit {
        id: EDIT_A_4,
        name: Cow::Borrowed("Unset Relation Fields"),
        authors: vec![AUTHOR_2],
        created_at: 1700000008,
        ops: vec![Op::UpdateRelation(UpdateRelation {
            id: RELATION_2,
            from_space: None,
            from_version: None,
            to_space: None,
            to_version: None,
            position: None,
            unset: vec![
                UnsetRelationField::FromSpace,
                UnsetRelationField::FromVersion,
                UnsetRelationField::ToSpace,
                UnsetRelationField::ToVersion,
                UnsetRelationField::Position,
            ],
            context: None,
        })],
    };

    grc_20::encode_edit(&edit).expect("Failed to encode edit")
}

/// Edits that squash values to ensure only the last op wins.
///
/// Edit for: QmSpaceBEdit2SquashOps (space B)
fn squash_ops_edit() -> Vec<u8> {
    let edit = Edit {
        id: EDIT_B_2,
        name: Cow::Borrowed("Squash Ops"),
        authors: vec![AUTHOR_1],
        created_at: 1700000009,
        ops: vec![
            Op::UpdateEntity(UpdateEntity {
                id: ENTITY_SQUASH,
                set_properties: vec![PropertyValue {
                    property: PROPERTY_NAME,
                    value: Grc20Value::Text {
                        value: Cow::Borrowed("First"),
                        language: None,
                    },
                }],
                unset_values: vec![],
                context: None,
            }),
            Op::UpdateEntity(UpdateEntity {
                id: ENTITY_SQUASH,
                set_properties: vec![PropertyValue {
                    property: PROPERTY_NAME,
                    value: Grc20Value::Text {
                        value: Cow::Borrowed("Second"),
                        language: None,
                    },
                }],
                unset_values: vec![],
                context: None,
            }),
            Op::UpdateEntity(UpdateEntity {
                id: ENTITY_SQUASH,
                set_properties: vec![],
                unset_values: vec![UnsetValue {
                    property: PROPERTY_NAME,
                    language: grc_20::UnsetLanguage::All,
                }],
                context: None,
            }),
            Op::UpdateEntity(UpdateEntity {
                id: ENTITY_SQUASH,
                set_properties: vec![PropertyValue {
                    property: PROPERTY_NAME,
                    value: Grc20Value::Text {
                        value: Cow::Borrowed("Final"),
                        language: None,
                    },
                }],
                unset_values: vec![],
                context: None,
            }),
        ],
    };

    grc_20::encode_edit(&edit).expect("Failed to encode edit")
}

/// Deletes a relation to verify delete handling.
///
/// Edit for: QmSpaceBEdit3DeleteRelation (space B)
fn delete_relation_edit() -> Vec<u8> {
    let edit = Edit {
        id: EDIT_B_3,
        name: Cow::Borrowed("Delete Relation"),
        authors: vec![AUTHOR_1],
        created_at: 1700000010,
        ops: vec![
            Op::CreateRelation(CreateRelation {
                id: RELATION_3,
                entity: Some(ENTITY_PROJECT_1),
                relation_type: RELATION_TYPE_RELATED_TO,
                from: ENTITY_PROJECT_1,
                from_is_value_ref: false,
                from_space: Some(SPACE_B),
                from_version: Some(EDIT_B_1),
                to: ENTITY_DOC_1,
                to_is_value_ref: false,
                to_space: Some(SPACE_B),
                to_version: Some(EDIT_B_1),
                position: Some(Cow::Borrowed("0")),
                context: None,
            }),
            Op::DeleteRelation(DeleteRelation {
                id: RELATION_3,
                context: None,
            }),
        ],
    };

    grc_20::encode_edit(&edit).expect("Failed to encode edit")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_has_all_mock_edits() {
        let cache = MockIpfsCache::new();
        let space_id = vec![0x01; 16];

        // All mock edits from mock_events.rs should be present
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
        assert!(cache.get("QmSpaceAEdit1CreateOrg", &space_id).await.is_ok());
        assert!(
            cache
                .get("QmSpaceAEdit2CreateRelations", &space_id)
                .await
                .is_ok()
        );
        assert!(cache.get("QmSpaceBEdit1CreateDoc", &space_id).await.is_ok());
        assert!(cache.get("QmSpaceBEdit2SquashOps", &space_id).await.is_ok());
        assert!(cache.get("QmSpaceBEdit3DeleteRelation", &space_id).await.is_ok());
        assert!(cache.get("QmSpaceCEdit1CreateTopic", &space_id).await.is_ok());
        assert!(cache.get("QmRootEdit3DeleteName", &space_id).await.is_ok());
        assert!(
            cache
                .get("QmSpaceAEdit3UpdateRelations", &space_id)
                .await
                .is_ok()
        );
        assert!(
            cache
                .get("QmSpaceAEdit4UnsetRelationFields", &space_id)
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

        let payload = result.payload.unwrap();
        let decoded = grc_20::decode_edit(&payload).expect("Failed to decode payload");
        assert_eq!(decoded.name, "Create Persons");
        assert_eq!(decoded.ops.len(), 2);
        assert_eq!(decoded.authors.len(), 1);
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
