//! Deterministic test topology for reproducible integration tests.
//!
//! This module provides well-known space and topic IDs, along with a
//! pre-defined graph topology that can be used across the entire system
//! for consistent testing.
//!
//! ## Topology Overview
//!
//! The deterministic topology creates:
//! - **11 canonical spaces** reachable from the Root space
//! - **7 non-canonical spaces** in isolated islands
//! - **Topic edges** demonstrating resolution behavior
//!
//! ```text
//! Canonical Graph:
//!
//! Root (0x01)
//!  ├─verified─▶ A (0x0A) ─verified─▶ C (0x0C) ─verified─▶ F (0x0F)
//!  │             │                    └─related─▶ G (0x10)
//!  │             └─related─▶ D (0x0D)
//!  ├─verified─▶ B (0x0B) ─verified─▶ E (0x0E)
//!  │             └─topic[T_H]─▶ H (0x11, already canonical via explicit edge)
//!  └─related─▶ H (0x11) ─verified─▶ I (0x12)
//!                        └─verified─▶ J (0x13)
//!
//! Non-Canonical Islands:
//!
//! Island 1: X (0x20) ─verified─▶ Y (0x21) ─verified─▶ Z (0x22)
//!            └─related─▶ W (0x23)
//!
//! Island 2: P (0x30) ─verified─▶ Q (0x31)
//!            └─topic[T_Q]─▶ Q
//!
//! Island 3: S (0x40)  [isolated]
//! ```

use crate::events::*;
use crate::generator::MockSubstream;

// =============================================================================
// Well-Known Space IDs
// =============================================================================

/// The root space - the source of canonicality.
pub const ROOT_SPACE_ID: SpaceId = make_id(0x01);

/// Root space's topic ID.
pub const ROOT_TOPIC_ID: TopicId = make_id(0x02);

// Canonical spaces (reachable from Root)
pub const SPACE_A: SpaceId = make_id(0x0A);
pub const SPACE_B: SpaceId = make_id(0x0B);
pub const SPACE_C: SpaceId = make_id(0x0C);
pub const SPACE_D: SpaceId = make_id(0x0D);
pub const SPACE_E: SpaceId = make_id(0x0E);
pub const SPACE_F: SpaceId = make_id(0x0F);
pub const SPACE_G: SpaceId = make_id(0x10);
pub const SPACE_H: SpaceId = make_id(0x11);
pub const SPACE_I: SpaceId = make_id(0x12);
pub const SPACE_J: SpaceId = make_id(0x13);

// Non-canonical spaces - Island 1
pub const SPACE_X: SpaceId = make_id(0x20);
pub const SPACE_Y: SpaceId = make_id(0x21);
pub const SPACE_Z: SpaceId = make_id(0x22);
pub const SPACE_W: SpaceId = make_id(0x23);

// Non-canonical spaces - Island 2
pub const SPACE_P: SpaceId = make_id(0x30);
pub const SPACE_Q: SpaceId = make_id(0x31);

// Non-canonical spaces - Island 3 (isolated)
pub const SPACE_S: SpaceId = make_id(0x40);

// =============================================================================
// Well-Known Topic IDs
// =============================================================================

/// Topic for space A.
pub const TOPIC_A: TopicId = make_id(0x8A);

/// Topic for space B.
pub const TOPIC_B: TopicId = make_id(0x8B);

/// Topic for space C.
pub const TOPIC_C: TopicId = make_id(0x8C);

/// Topic for space D.
pub const TOPIC_D: TopicId = make_id(0x8D);

/// Topic for space E.
pub const TOPIC_E: TopicId = make_id(0x8E);

/// Topic for space F.
pub const TOPIC_F: TopicId = make_id(0x8F);

/// Topic for space G.
pub const TOPIC_G: TopicId = make_id(0x90);

/// Topic for space H.
pub const TOPIC_H: TopicId = make_id(0x91);

/// Topic for space I.
pub const TOPIC_I: TopicId = make_id(0x92);

/// Topic for space J.
pub const TOPIC_J: TopicId = make_id(0x93);

/// Topic for space X.
pub const TOPIC_X: TopicId = make_id(0xA0);

/// Topic for space Y.
pub const TOPIC_Y: TopicId = make_id(0xA1);

/// Topic for space Z.
pub const TOPIC_Z: TopicId = make_id(0xA2);

/// Topic for space W.
pub const TOPIC_W: TopicId = make_id(0xA3);

/// Topic for space P.
pub const TOPIC_P: TopicId = make_id(0xB0);

/// Topic for space Q.
pub const TOPIC_Q: TopicId = make_id(0xB1);

/// Topic for space S.
pub const TOPIC_S: TopicId = make_id(0xC0);

/// A shared topic announced by multiple spaces (C, G, Y).
pub const TOPIC_SHARED: TopicId = make_id(0xF0);

// =============================================================================
// Well-Known Addresses
// =============================================================================

/// Address for the root space owner.
pub const ROOT_OWNER: Address = make_address(0x01);

/// Address for test user 1.
pub const USER_1: Address = make_address(0x11);

/// Address for test user 2.
pub const USER_2: Address = make_address(0x12);

/// Address for test user 3.
pub const USER_3: Address = make_address(0x13);

// =============================================================================
// Well-Known Edit IDs
// =============================================================================

// Edits for Root space
pub const EDIT_ROOT_1: EditId = make_id(0xE1);
pub const EDIT_ROOT_2: EditId = make_id(0xE2);

// Edits for Space A
pub const EDIT_A_1: EditId = make_id(0xEA);
pub const EDIT_A_2: EditId = make_id(0xEB);

// Edits for Space B
pub const EDIT_B_1: EditId = make_id(0xEC);

// Edits for Space C
pub const EDIT_C_1: EditId = make_id(0xED);

// =============================================================================
// Well-Known Entity IDs
// =============================================================================

/// Entity representing a "Person" type in Root space.
pub const ENTITY_PERSON_1: EntityId = make_id(0xF1);

/// Entity representing a "Person" type in Root space.
pub const ENTITY_PERSON_2: EntityId = make_id(0xF2);

/// Entity representing an "Organization" in Space A.
pub const ENTITY_ORG_1: EntityId = make_id(0xF3);

/// Entity representing a "Project" in Space A.
pub const ENTITY_PROJECT_1: EntityId = make_id(0xF4);

/// Entity representing a "Document" in Space B.
pub const ENTITY_DOC_1: EntityId = make_id(0xF5);

/// Entity representing a "Topic" in Space C.
pub const ENTITY_TOPIC_1: EntityId = make_id(0xF6);

// =============================================================================
// Well-Known Property IDs
// =============================================================================

/// Property for "name" (string).
pub const PROPERTY_NAME: PropertyId = make_id(0xD1);

/// Property for "description" (string).
pub const PROPERTY_DESCRIPTION: PropertyId = make_id(0xD2);

/// Property for "url" (string).
pub const PROPERTY_URL: PropertyId = make_id(0xD3);

/// Property for "created_at" (time).
pub const PROPERTY_CREATED_AT: PropertyId = make_id(0xD4);

/// Property for "count" (number).
pub const PROPERTY_COUNT: PropertyId = make_id(0xD5);

// =============================================================================
// Well-Known Relation Type IDs
// =============================================================================

/// Relation type for "authored_by".
pub const RELATION_TYPE_AUTHORED_BY: RelationTypeId = make_id(0xC1);

/// Relation type for "belongs_to".
pub const RELATION_TYPE_BELONGS_TO: RelationTypeId = make_id(0xC2);

/// Relation type for "related_to".
pub const RELATION_TYPE_RELATED_TO: RelationTypeId = make_id(0xC3);

// =============================================================================
// Well-Known Relation IDs
// =============================================================================

/// Relation: Person 1 authored by Org 1.
pub const RELATION_1: RelationId = make_id(0xB1);

/// Relation: Project 1 belongs to Org 1.
pub const RELATION_2: RelationId = make_id(0xB2);

// =============================================================================
// Topology Generation
// =============================================================================

/// Generate the deterministic test topology.
///
/// Returns a list of mock blocks containing all space creations, trust
/// extensions, and edits needed to build the test graph.
///
/// The topology includes:
/// - 18 space creations (11 canonical + 7 non-canonical)
/// - 14 explicit trust edges
/// - 5 topic-based trust edges
/// - 6 edits with various GRC-20 operations
pub fn generate() -> Vec<MockBlock> {
    let mut mock = MockSubstream::deterministic();
    let mut blocks = Vec::new();

    // =========================================================================
    // Phase 1: Create all spaces
    // =========================================================================

    // Root space (personal)
    let root = mock.create_personal_space(ROOT_SPACE_ID, ROOT_TOPIC_ID, ROOT_OWNER);
    blocks.push(mock.block_with_events(vec![MockEvent::SpaceCreated(root)]));

    // Canonical spaces
    let spaces = [
        (SPACE_A, TOPIC_A, SpaceType::Personal { owner: USER_1 }),
        (SPACE_B, TOPIC_B, SpaceType::Personal { owner: USER_2 }),
        (SPACE_C, TOPIC_C, SpaceType::Personal { owner: USER_1 }),
        (SPACE_D, TOPIC_D, SpaceType::Personal { owner: USER_2 }),
        (SPACE_E, TOPIC_E, SpaceType::Personal { owner: USER_3 }),
        (SPACE_F, TOPIC_F, SpaceType::Personal { owner: USER_1 }),
        (SPACE_G, TOPIC_G, SpaceType::Personal { owner: USER_2 }),
        (SPACE_H, TOPIC_H, SpaceType::Personal { owner: USER_3 }),
        (SPACE_I, TOPIC_I, SpaceType::Personal { owner: USER_1 }),
        (SPACE_J, TOPIC_J, SpaceType::Personal { owner: USER_2 }),
    ];

    for (space_id, topic_id, space_type) in spaces {
        let event = mock.create_space(space_id, topic_id, space_type);
        blocks.push(mock.block_with_events(vec![MockEvent::SpaceCreated(event)]));
    }

    // Non-canonical spaces - Island 1
    let island1_spaces = [
        (SPACE_X, TOPIC_X, SpaceType::Personal { owner: USER_1 }),
        (SPACE_Y, TOPIC_Y, SpaceType::Personal { owner: USER_2 }),
        (SPACE_Z, TOPIC_Z, SpaceType::Personal { owner: USER_3 }),
        (SPACE_W, TOPIC_W, SpaceType::Personal { owner: USER_1 }),
    ];

    for (space_id, topic_id, space_type) in island1_spaces {
        let event = mock.create_space(space_id, topic_id, space_type);
        blocks.push(mock.block_with_events(vec![MockEvent::SpaceCreated(event)]));
    }

    // Non-canonical spaces - Island 2
    let island2_spaces = [
        (SPACE_P, TOPIC_P, SpaceType::Dao {
            initial_editors: vec![SPACE_Q],
            initial_members: vec![],
        }),
        (SPACE_Q, TOPIC_Q, SpaceType::Personal { owner: USER_2 }),
    ];

    for (space_id, topic_id, space_type) in island2_spaces {
        let event = mock.create_space(space_id, topic_id, space_type);
        blocks.push(mock.block_with_events(vec![MockEvent::SpaceCreated(event)]));
    }

    // Non-canonical spaces - Island 3 (isolated)
    let s = mock.create_personal_space(SPACE_S, TOPIC_S, USER_3);
    blocks.push(mock.block_with_events(vec![MockEvent::SpaceCreated(s)]));

    // =========================================================================
    // Phase 2: Create trust edges (canonical graph)
    // =========================================================================

    // Root's direct children
    let root_to_a = mock.extend_verified(ROOT_SPACE_ID, SPACE_A);
    let root_to_b = mock.extend_verified(ROOT_SPACE_ID, SPACE_B);
    let root_to_h = mock.extend_related(ROOT_SPACE_ID, SPACE_H);
    blocks.push(mock.block_with_events(vec![
        MockEvent::TrustExtended(root_to_a),
        MockEvent::TrustExtended(root_to_b),
        MockEvent::TrustExtended(root_to_h),
    ]));

    // A's children
    let a_to_c = mock.extend_verified(SPACE_A, SPACE_C);
    let a_to_d = mock.extend_related(SPACE_A, SPACE_D);
    blocks.push(mock.block_with_events(vec![
        MockEvent::TrustExtended(a_to_c),
        MockEvent::TrustExtended(a_to_d),
    ]));

    // B's children
    let b_to_e = mock.extend_verified(SPACE_B, SPACE_E);
    blocks.push(mock.block_with_events(vec![MockEvent::TrustExtended(b_to_e)]));

    // C's children
    let c_to_f = mock.extend_verified(SPACE_C, SPACE_F);
    let c_to_g = mock.extend_related(SPACE_C, SPACE_G);
    blocks.push(mock.block_with_events(vec![
        MockEvent::TrustExtended(c_to_f),
        MockEvent::TrustExtended(c_to_g),
    ]));

    // H's children
    let h_to_i = mock.extend_verified(SPACE_H, SPACE_I);
    let h_to_j = mock.extend_verified(SPACE_H, SPACE_J);
    blocks.push(mock.block_with_events(vec![
        MockEvent::TrustExtended(h_to_i),
        MockEvent::TrustExtended(h_to_j),
    ]));

    // =========================================================================
    // Phase 3: Create trust edges (non-canonical islands)
    // =========================================================================

    // Island 1
    let x_to_y = mock.extend_verified(SPACE_X, SPACE_Y);
    let x_to_w = mock.extend_related(SPACE_X, SPACE_W);
    let y_to_z = mock.extend_verified(SPACE_Y, SPACE_Z);
    blocks.push(mock.block_with_events(vec![
        MockEvent::TrustExtended(x_to_y),
        MockEvent::TrustExtended(x_to_w),
        MockEvent::TrustExtended(y_to_z),
    ]));

    // Island 2
    let p_to_q = mock.extend_verified(SPACE_P, SPACE_Q);
    blocks.push(mock.block_with_events(vec![MockEvent::TrustExtended(p_to_q)]));

    // =========================================================================
    // Phase 4: Topic-based trust edges
    // =========================================================================

    // B -> topic of H (H is already canonical via Root -> H)
    let b_topic_h = mock.extend_subtopic(SPACE_B, TOPIC_H);
    blocks.push(mock.block_with_events(vec![MockEvent::TrustExtended(b_topic_h)]));

    // Root -> topic of E
    let root_topic_e = mock.extend_subtopic(ROOT_SPACE_ID, TOPIC_E);
    blocks.push(mock.block_with_events(vec![MockEvent::TrustExtended(root_topic_e)]));

    // A -> shared topic (resolves to C and G, both canonical)
    let a_topic_shared = mock.extend_subtopic(SPACE_A, TOPIC_SHARED);
    blocks.push(mock.block_with_events(vec![MockEvent::TrustExtended(a_topic_shared)]));

    // X -> topic of A (X is non-canonical, but points to canonical A)
    let x_topic_a = mock.extend_subtopic(SPACE_X, TOPIC_A);
    blocks.push(mock.block_with_events(vec![MockEvent::TrustExtended(x_topic_a)]));

    // P -> topic of Q (both non-canonical)
    let p_topic_q = mock.extend_subtopic(SPACE_P, TOPIC_Q);
    blocks.push(mock.block_with_events(vec![MockEvent::TrustExtended(p_topic_q)]));

    // =========================================================================
    // Phase 5: Create edits with GRC-20 operations
    // =========================================================================

    // Edit 1 in Root: Create two Person entities with names
    let edit_root_1 = mock.publish_edit(
        EDIT_ROOT_1,
        ROOT_SPACE_ID,
        vec![ROOT_OWNER],
        "Create initial persons".to_string(),
        vec![
            // Define the name property
            Op::CreateProperty(CreateProperty {
                id: PROPERTY_NAME,
                data_type: DataType::String,
            }),
            // Create Person 1 with a name
            Op::UpdateEntity(UpdateEntity {
                id: ENTITY_PERSON_1,
                values: vec![Value {
                    property: PROPERTY_NAME,
                    value: "Alice".to_string(),
                }],
            }),
            // Create Person 2 with a name
            Op::UpdateEntity(UpdateEntity {
                id: ENTITY_PERSON_2,
                values: vec![Value {
                    property: PROPERTY_NAME,
                    value: "Bob".to_string(),
                }],
            }),
        ],
    );
    blocks.push(mock.block_with_events(vec![MockEvent::EditPublished(edit_root_1)]));

    // Edit 2 in Root: Add descriptions to persons
    let edit_root_2 = mock.publish_edit(
        EDIT_ROOT_2,
        ROOT_SPACE_ID,
        vec![ROOT_OWNER],
        "Add descriptions".to_string(),
        vec![
            // Define the description property
            Op::CreateProperty(CreateProperty {
                id: PROPERTY_DESCRIPTION,
                data_type: DataType::String,
            }),
            // Update Person 1 with description
            Op::UpdateEntity(UpdateEntity {
                id: ENTITY_PERSON_1,
                values: vec![Value {
                    property: PROPERTY_DESCRIPTION,
                    value: "A software engineer".to_string(),
                }],
            }),
        ],
    );
    blocks.push(mock.block_with_events(vec![MockEvent::EditPublished(edit_root_2)]));

    // Edit 1 in Space A: Create an Organization and Project
    let edit_a_1 = mock.publish_edit(
        EDIT_A_1,
        SPACE_A,
        vec![USER_1],
        "Create organization".to_string(),
        vec![
            // Create Organization entity
            Op::UpdateEntity(UpdateEntity {
                id: ENTITY_ORG_1,
                values: vec![Value {
                    property: PROPERTY_NAME,
                    value: "Acme Corp".to_string(),
                }],
            }),
            // Create Project entity
            Op::UpdateEntity(UpdateEntity {
                id: ENTITY_PROJECT_1,
                values: vec![Value {
                    property: PROPERTY_NAME,
                    value: "Project Alpha".to_string(),
                }],
            }),
        ],
    );
    blocks.push(mock.block_with_events(vec![MockEvent::EditPublished(edit_a_1)]));

    // Edit 2 in Space A: Create relations between entities
    let edit_a_2 = mock.publish_edit(
        EDIT_A_2,
        SPACE_A,
        vec![USER_1],
        "Create relations".to_string(),
        vec![
            // Project belongs to Organization
            Op::CreateRelation(CreateRelation {
                id: RELATION_2,
                relation_type: RELATION_TYPE_BELONGS_TO,
                from_entity: ENTITY_PROJECT_1,
                from_space: Some(SPACE_A),
                to_entity: ENTITY_ORG_1,
                to_space: Some(SPACE_A),
                entity: make_id(0xB3), // Relation entity for storing properties
                position: None,
                verified: Some(true),
            }),
        ],
    );
    blocks.push(mock.block_with_events(vec![MockEvent::EditPublished(edit_a_2)]));

    // Edit 1 in Space B: Create a Document
    let edit_b_1 = mock.publish_edit(
        EDIT_B_1,
        SPACE_B,
        vec![USER_2],
        "Create document".to_string(),
        vec![
            Op::UpdateEntity(UpdateEntity {
                id: ENTITY_DOC_1,
                values: vec![
                    Value {
                        property: PROPERTY_NAME,
                        value: "Technical Specification".to_string(),
                    },
                    Value {
                        property: PROPERTY_URL,
                        value: "https://example.com/spec".to_string(),
                    },
                ],
            }),
        ],
    );
    blocks.push(mock.block_with_events(vec![MockEvent::EditPublished(edit_b_1)]));

    // Edit 1 in Space C: Create a Topic with cross-space relation
    let edit_c_1 = mock.publish_edit(
        EDIT_C_1,
        SPACE_C,
        vec![USER_1],
        "Create topic with relation".to_string(),
        vec![
            Op::UpdateEntity(UpdateEntity {
                id: ENTITY_TOPIC_1,
                values: vec![Value {
                    property: PROPERTY_NAME,
                    value: "Knowledge Graphs".to_string(),
                }],
            }),
            // Relation to Document in Space B (cross-space relation)
            Op::CreateRelation(CreateRelation {
                id: RELATION_1,
                relation_type: RELATION_TYPE_RELATED_TO,
                from_entity: ENTITY_TOPIC_1,
                from_space: Some(SPACE_C),
                to_entity: ENTITY_DOC_1,
                to_space: Some(SPACE_B), // Cross-space reference
                entity: make_id(0xB4),
                position: None,
                verified: Some(true),
            }),
        ],
    );
    blocks.push(mock.block_with_events(vec![MockEvent::EditPublished(edit_c_1)]));

    blocks
}

/// Get the list of all canonical space IDs.
pub fn canonical_spaces() -> Vec<SpaceId> {
    vec![
        ROOT_SPACE_ID,
        SPACE_A,
        SPACE_B,
        SPACE_C,
        SPACE_D,
        SPACE_E,
        SPACE_F,
        SPACE_G,
        SPACE_H,
        SPACE_I,
        SPACE_J,
    ]
}

/// Get the list of all non-canonical space IDs.
pub fn non_canonical_spaces() -> Vec<SpaceId> {
    vec![
        SPACE_X,
        SPACE_Y,
        SPACE_Z,
        SPACE_W,
        SPACE_P,
        SPACE_Q,
        SPACE_S,
    ]
}

/// Get all space IDs in the topology.
pub fn all_spaces() -> Vec<SpaceId> {
    let mut spaces = canonical_spaces();
    spaces.extend(non_canonical_spaces());
    spaces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_topology() {
        let blocks = generate();

        // Should have multiple blocks
        assert!(!blocks.is_empty());

        // Count events by type
        let mut space_count = 0;
        let mut trust_count = 0;
        let mut edit_count = 0;

        for block in &blocks {
            for event in &block.events {
                match event {
                    MockEvent::SpaceCreated(_) => space_count += 1,
                    MockEvent::TrustExtended(_) => trust_count += 1,
                    MockEvent::EditPublished(_) => edit_count += 1,
                }
            }
        }

        // 18 spaces: 11 canonical + 7 non-canonical
        assert_eq!(space_count, 18);

        // 14 explicit edges + 5 topic edges = 19 trust extensions
        assert_eq!(trust_count, 19);

        // 6 edits: 2 in Root, 2 in A, 1 in B, 1 in C
        assert_eq!(edit_count, 6);
    }

    #[test]
    fn test_edits_have_valid_structure() {
        let blocks = generate();

        let edits: Vec<&EditPublished> = blocks
            .iter()
            .flat_map(|b| &b.events)
            .filter_map(|e| match e {
                MockEvent::EditPublished(edit) => Some(edit),
                _ => None,
            })
            .collect();

        // All edits should have at least one op
        for edit in &edits {
            assert!(!edit.ops.is_empty(), "Edit {} has no ops", edit.name);
            assert!(!edit.authors.is_empty(), "Edit {} has no authors", edit.name);
        }

        // Verify specific edits exist
        let edit_ids: Vec<EditId> = edits.iter().map(|e| e.edit_id).collect();
        assert!(edit_ids.contains(&EDIT_ROOT_1));
        assert!(edit_ids.contains(&EDIT_ROOT_2));
        assert!(edit_ids.contains(&EDIT_A_1));
        assert!(edit_ids.contains(&EDIT_A_2));
        assert!(edit_ids.contains(&EDIT_B_1));
        assert!(edit_ids.contains(&EDIT_C_1));
    }

    #[test]
    fn test_edits_in_correct_spaces() {
        let blocks = generate();

        for block in &blocks {
            for event in &block.events {
                if let MockEvent::EditPublished(edit) = event {
                    match edit.edit_id {
                        id if id == EDIT_ROOT_1 || id == EDIT_ROOT_2 => {
                            assert_eq!(edit.space_id, ROOT_SPACE_ID);
                        }
                        id if id == EDIT_A_1 || id == EDIT_A_2 => {
                            assert_eq!(edit.space_id, SPACE_A);
                        }
                        id if id == EDIT_B_1 => {
                            assert_eq!(edit.space_id, SPACE_B);
                        }
                        id if id == EDIT_C_1 => {
                            assert_eq!(edit.space_id, SPACE_C);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    #[test]
    fn test_canonical_spaces_count() {
        assert_eq!(canonical_spaces().len(), 11);
    }

    #[test]
    fn test_non_canonical_spaces_count() {
        assert_eq!(non_canonical_spaces().len(), 7);
    }

    #[test]
    fn test_all_spaces_count() {
        assert_eq!(all_spaces().len(), 18);
    }

    #[test]
    fn test_space_ids_are_unique() {
        let spaces = all_spaces();
        let mut seen = std::collections::HashSet::new();
        for space in spaces {
            assert!(seen.insert(space), "Duplicate space ID found");
        }
    }
}
