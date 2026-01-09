//! Mock IPFS cache for development/testing.
//!
//! Pre-populated with edit content matching the IPFS hashes emitted by
//! hermes-relay's mock test topology (see `hermes-relay/src/source/mock_events.rs`).

use std::collections::HashMap;

use async_trait::async_trait;
use hermes_instrumentation::debug;
use uuid::uuid;
use wire::pb::grc20::{Edit, Entity, Op, Relation, Value, op::Payload};

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

// Property IDs (from sdk/src/core/ids.rs)
const PROPERTY_NAME: [u8; 16] = *uuid!("a126ca53-0c8e-48d5-b888-82c734c38935").as_bytes();
const PROPERTY_DESCRIPTION: [u8; 16] = *uuid!("9b1f76ff-9711-404c-861e-59dc3fa7d037").as_bytes();

#[allow(dead_code)]
const PROPERTY_URL: [u8; 16] = make_id(0xD3);

// Relation Type IDs
const RELATION_TYPE_BELONGS_TO: [u8; 16] = make_id(0xC2);
#[allow(dead_code)]
const RELATION_TYPE_RELATED_TO: [u8; 16] = make_id(0xC3);

/// The special "type" relation type ID that the search indexer processes.
const TYPE_RELATION_TYPE_ID: [u8; 16] = *uuid!("8f151ba4-de20-4e3c-9cb4-99ddf96f48f1").as_bytes();

// Type Entity IDs (entities that represent types)
const TYPE_PERSON: [u8; 16] = make_id(0xB1);
const TYPE_ORGANIZATION: [u8; 16] = make_id(0xB2);
const TYPE_PROJECT: [u8; 16] = make_id(0xB3);

// Type Relation IDs (for type relations that assign types to entities)
const TYPE_RELATION_1: [u8; 16] = make_id(0xC4); // Person1 -> PersonType
const TYPE_RELATION_2: [u8; 16] = make_id(0xC5); // Person2 -> PersonType
const TYPE_RELATION_3: [u8; 16] = make_id(0xC6); // Org1 -> OrgType
const TYPE_RELATION_4: [u8; 16] = make_id(0xC7); // Project1 -> ProjectType (will be deleted)
const TYPE_RELATION_5: [u8; 16] = make_id(0xC8); // Project1 -> OrgType (secondary, survives delete)
const TYPE_RELATION_6: [u8; 16] = make_id(0xC9); // Person1 -> OrgType (Alice is also an Organization founder)

// Edit IDs
const EDIT_ROOT_1: [u8; 16] = make_id(0xE1);
const EDIT_ROOT_2: [u8; 16] = make_id(0xE2);
const EDIT_ROOT_3: [u8; 16] = make_id(0xE3); // Create type entities
const EDIT_ROOT_4: [u8; 16] = make_id(0xE4); // Create type relations
const EDIT_ROOT_5: [u8; 16] = make_id(0xE5); // Delete type relation
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

/// Helper to format a byte slice as a UUID string if it's 16 bytes.
fn format_id(bytes: &[u8]) -> String {
    if bytes.len() == 16 {
        uuid::Uuid::from_slice(bytes)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| hex::encode(bytes))
    } else {
        hex::encode(bytes)
    }
}

/// Log details about an edit being fetched from the mock cache.
fn log_edit_details(ipfs_hash: &str, edit: &Edit) {
    debug!(
        ipfs_hash = %ipfs_hash,
        edit_name = %edit.name,
        ops_count = edit.ops.len(),
        "Mock cache: fetching edit"
    );

    for (i, op) in edit.ops.iter().enumerate() {
        if let Some(ref payload) = op.payload {
            match payload {
                Payload::UpdateEntity(entity) => {
                    let entity_id = format_id(&entity.id);
                    let name_value = entity
                        .values
                        .iter()
                        .find(|v| v.property == PROPERTY_NAME)
                        .map(|v| v.value.as_str())
                        .unwrap_or("<no name>");
                    debug!(
                        op_index = i,
                        entity_id = %entity_id,
                        name = %name_value,
                        values_count = entity.values.len(),
                        "  -> UpdateEntity"
                    );
                }
                Payload::CreateRelation(relation) => {
                    let relation_id = format_id(&relation.id);
                    let relation_type = format_id(&relation.r#type);
                    let from_entity = format_id(&relation.from_entity);
                    let to_entity = format_id(&relation.to_entity);
                    debug!(
                        op_index = i,
                        relation_id = %relation_id,
                        relation_type = %relation_type,
                        from_entity = %from_entity,
                        to_entity = %to_entity,
                        "  -> CreateRelation"
                    );
                }
                Payload::DeleteRelation(relation_id) => {
                    let id = format_id(relation_id);
                    debug!(
                        op_index = i,
                        relation_id = %id,
                        "  -> DeleteRelation"
                    );
                }
                _ => {
                    debug!(op_index = i, "  -> Other operation");
                }
            }
        }
    }
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
        // Type-related edits (for search indexer testing)
        self.edits
            .insert("QmRootEdit3CreateTypes".into(), create_types_edit());
        self.edits.insert(
            "QmRootEdit4CreateTypeRelations".into(),
            create_type_relations_edit(),
        );
        self.edits.insert(
            "QmRootEdit5DeleteTypeRelation".into(),
            delete_type_relation_edit(),
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
            Some(edit) => {
                // Log the edit details
                log_edit_details(ipfs_hash, edit);
                Ok(CachedEdit::success(
                    ipfs_hash.to_string(),
                    edit.clone(),
                    space_id.to_vec(),
                ))
            }
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

// =============================================================================
// Type relation edits (for search indexer testing)
// =============================================================================

/// Creates type entities (Person, Organization, Project types).
///
/// Edit for: QmRootEdit3CreateTypes (root space)
fn create_types_edit() -> Edit {
    Edit {
        id: EDIT_ROOT_3.to_vec(),
        name: "Create Types".into(),
        ops: vec![
            // Create Person type
            Op {
                payload: Some(wire::pb::grc20::op::Payload::UpdateEntity(Entity {
                    id: TYPE_PERSON.to_vec(),
                    values: vec![
                        Value {
                            property: PROPERTY_NAME.to_vec(),
                            value: "Person".into(),
                            options: None,
                        },
                        Value {
                            property: PROPERTY_DESCRIPTION.to_vec(),
                            value: "A human being".into(),
                            options: None,
                        },
                    ],
                })),
            },
            // Create Organization type
            Op {
                payload: Some(wire::pb::grc20::op::Payload::UpdateEntity(Entity {
                    id: TYPE_ORGANIZATION.to_vec(),
                    values: vec![
                        Value {
                            property: PROPERTY_NAME.to_vec(),
                            value: "Organization".into(),
                            options: None,
                        },
                        Value {
                            property: PROPERTY_DESCRIPTION.to_vec(),
                            value: "A structured group of people".into(),
                            options: None,
                        },
                    ],
                })),
            },
            // Create Project type
            Op {
                payload: Some(wire::pb::grc20::op::Payload::UpdateEntity(Entity {
                    id: TYPE_PROJECT.to_vec(),
                    values: vec![
                        Value {
                            property: PROPERTY_NAME.to_vec(),
                            value: "Project".into(),
                            options: None,
                        },
                        Value {
                            property: PROPERTY_DESCRIPTION.to_vec(),
                            value: "A planned endeavor".into(),
                            options: None,
                        },
                    ],
                })),
            },
        ],
        authors: vec![AUTHOR_1.to_vec()],
        language: None,
    }
}

/// Creates type relations using TYPE_RELATION_TYPE_ID.
/// These relations assign types to entities and should be indexed by the search indexer.
///
/// Edit for: QmRootEdit4CreateTypeRelations (root space)
fn create_type_relations_edit() -> Edit {
    Edit {
        id: EDIT_ROOT_4.to_vec(),
        name: "Create Type Relations".into(),
        ops: vec![
            // Person 1 has type Person
            Op {
                payload: Some(wire::pb::grc20::op::Payload::CreateRelation(Relation {
                    id: TYPE_RELATION_1.to_vec(),
                    r#type: TYPE_RELATION_TYPE_ID.to_vec(),
                    from_entity: ENTITY_PERSON_1.to_vec(),
                    from_space: None,
                    from_version: None,
                    to_entity: TYPE_PERSON.to_vec(),
                    to_space: None,
                    to_version: None,
                    entity: ENTITY_PERSON_1.to_vec(),
                    position: Some("0".into()),
                    verified: Some(true),
                })),
            },
            // Person 2 has type Person
            Op {
                payload: Some(wire::pb::grc20::op::Payload::CreateRelation(Relation {
                    id: TYPE_RELATION_2.to_vec(),
                    r#type: TYPE_RELATION_TYPE_ID.to_vec(),
                    from_entity: ENTITY_PERSON_2.to_vec(),
                    from_space: None,
                    from_version: None,
                    to_entity: TYPE_PERSON.to_vec(),
                    to_space: None,
                    to_version: None,
                    entity: ENTITY_PERSON_2.to_vec(),
                    position: Some("0".into()),
                    verified: Some(true),
                })),
            },
            // Organization 1 has type Organization
            Op {
                payload: Some(wire::pb::grc20::op::Payload::CreateRelation(Relation {
                    id: TYPE_RELATION_3.to_vec(),
                    r#type: TYPE_RELATION_TYPE_ID.to_vec(),
                    from_entity: ENTITY_ORG_1.to_vec(),
                    from_space: None,
                    from_version: None,
                    to_entity: TYPE_ORGANIZATION.to_vec(),
                    to_space: None,
                    to_version: None,
                    entity: ENTITY_ORG_1.to_vec(),
                    position: Some("0".into()),
                    verified: Some(true),
                })),
            },
            // Project 1 has type Project (this will be deleted in a later edit)
            Op {
                payload: Some(wire::pb::grc20::op::Payload::CreateRelation(Relation {
                    id: TYPE_RELATION_4.to_vec(),
                    r#type: TYPE_RELATION_TYPE_ID.to_vec(),
                    from_entity: ENTITY_PROJECT_1.to_vec(),
                    from_space: None,
                    from_version: None,
                    to_entity: TYPE_PROJECT.to_vec(),
                    to_space: None,
                    to_version: None,
                    entity: ENTITY_PROJECT_1.to_vec(),
                    position: Some("0".into()),
                    verified: Some(true),
                })),
            },
            // Project 1 also has type Organization (secondary relation that survives the delete)
            Op {
                payload: Some(wire::pb::grc20::op::Payload::CreateRelation(Relation {
                    id: TYPE_RELATION_5.to_vec(),
                    r#type: TYPE_RELATION_TYPE_ID.to_vec(),
                    from_entity: ENTITY_PROJECT_1.to_vec(),
                    from_space: None,
                    from_version: None,
                    to_entity: TYPE_ORGANIZATION.to_vec(),
                    to_space: None,
                    to_version: None,
                    entity: ENTITY_PROJECT_1.to_vec(),
                    position: Some("1".into()),
                    verified: Some(true),
                })),
            },
            // Person 1 (Alice) also has type Organization (she's an org founder, so has 2 types)
            Op {
                payload: Some(wire::pb::grc20::op::Payload::CreateRelation(Relation {
                    id: TYPE_RELATION_6.to_vec(),
                    r#type: TYPE_RELATION_TYPE_ID.to_vec(),
                    from_entity: ENTITY_PERSON_1.to_vec(),
                    from_space: None,
                    from_version: None,
                    to_entity: TYPE_ORGANIZATION.to_vec(),
                    to_space: None,
                    to_version: None,
                    entity: ENTITY_PERSON_1.to_vec(),
                    position: Some("1".into()),
                    verified: Some(true),
                })),
            },
        ],
        authors: vec![AUTHOR_1.to_vec()],
        language: None,
    }
}

/// Deletes a type relation (removes Project type from Project 1).
/// This tests the DeleteRelation operation for type relations.
///
/// Edit for: QmRootEdit5DeleteTypeRelation (root space)
fn delete_type_relation_edit() -> Edit {
    Edit {
        id: EDIT_ROOT_5.to_vec(),
        name: "Delete Type Relation".into(),
        ops: vec![
            // Delete the Project type relation from Project 1
            Op {
                payload: Some(wire::pb::grc20::op::Payload::DeleteRelation(
                    TYPE_RELATION_4.to_vec(),
                )),
            },
        ],
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

        // All 9 edits from mock_events.rs should be present
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
        // Type-related edits for search indexer testing
        assert!(cache.get("QmRootEdit3CreateTypes", &space_id).await.is_ok());
        assert!(
            cache
                .get("QmRootEdit4CreateTypeRelations", &space_id)
                .await
                .is_ok()
        );
        assert!(
            cache
                .get("QmRootEdit5DeleteTypeRelation", &space_id)
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
