use std::sync::LazyLock;

use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;
use hermes_schema::pb::governance::{HermesProposalCreated, VotingMode as ProtoVotingMode};
use hermes_schema::pb::space::{hermes_create_space::Payload, HermesCreateSpace};
use sdk::core::ids::{
    CREATED_AT_BLOCK_PROPERTY_ID, DAO_SPACE_TYPE_ID, DESCRIPTION_PROPERTY_ID, EOA_SPACE_TYPE_ID,
    GEO_SYSTEM_NAMESPACE, NAME_PROPERTY_ID, PROPOSAL_TYPE_ID, SPACE_ADDRESS_PROPERTY_ID,
    SPACE_ID_PROPERTY_ID, SPACE_TYPE_ID, SYSTEM_TYPES_RELATION_TYPE_ID, SYSTEM_TYPE_ID,
    VOTING_MODE_PROPERTY_ID,
};
use uuid::Uuid;

use crate::error::HandlerError;
use crate::handlers::edits::derive_value_id;
use crate::models::{
    entities::EntityItem,
    relations::{RelationOp, SetRelationItem},
    values::{ValueChangeType, ValueOp},
};

static GEO_SYSTEM_NS: LazyLock<Uuid> = LazyLock::new(|| {
    Uuid::parse_str(GEO_SYSTEM_NAMESPACE).expect("GEO_SYSTEM_NAMESPACE is a valid UUID constant")
});

static SYSTEM_TYPES_RELATION_TID: LazyLock<Uuid> = LazyLock::new(|| {
    Uuid::parse_str(SYSTEM_TYPES_RELATION_TYPE_ID)
        .expect("SYSTEM_TYPES_RELATION_TYPE_ID is a valid UUID constant")
});

static SPACE_ADDRESS_PID: LazyLock<Uuid> = LazyLock::new(|| {
    Uuid::parse_str(SPACE_ADDRESS_PROPERTY_ID)
        .expect("SPACE_ADDRESS_PROPERTY_ID is a valid UUID constant")
});

static CREATED_AT_BLOCK_PID: LazyLock<Uuid> = LazyLock::new(|| {
    Uuid::parse_str(CREATED_AT_BLOCK_PROPERTY_ID)
        .expect("CREATED_AT_BLOCK_PROPERTY_ID is a valid UUID constant")
});

static NAME_PID: LazyLock<Uuid> = LazyLock::new(|| {
    Uuid::parse_str(NAME_PROPERTY_ID).expect("NAME_PROPERTY_ID is a valid UUID constant")
});

static DESCRIPTION_PID: LazyLock<Uuid> = LazyLock::new(|| {
    Uuid::parse_str(DESCRIPTION_PROPERTY_ID)
        .expect("DESCRIPTION_PROPERTY_ID is a valid UUID constant")
});

static SYSTEM_TYPE_UUID: LazyLock<Uuid> = LazyLock::new(|| {
    Uuid::parse_str(SYSTEM_TYPE_ID).expect("SYSTEM_TYPE_ID is a valid UUID constant")
});

static SPACE_TYPE_UUID: LazyLock<Uuid> = LazyLock::new(|| {
    Uuid::parse_str(SPACE_TYPE_ID).expect("SPACE_TYPE_ID is a valid UUID constant")
});

static EOA_SPACE_TYPE_UUID: LazyLock<Uuid> = LazyLock::new(|| {
    Uuid::parse_str(EOA_SPACE_TYPE_ID).expect("EOA_SPACE_TYPE_ID is a valid UUID constant")
});

static DAO_SPACE_TYPE_UUID: LazyLock<Uuid> = LazyLock::new(|| {
    Uuid::parse_str(DAO_SPACE_TYPE_ID).expect("DAO_SPACE_TYPE_ID is a valid UUID constant")
});

static VOTING_MODE_PID: LazyLock<Uuid> = LazyLock::new(|| {
    Uuid::parse_str(VOTING_MODE_PROPERTY_ID)
        .expect("VOTING_MODE_PROPERTY_ID is a valid UUID constant")
});

static SPACE_ID_PROPERTY_PID: LazyLock<Uuid> = LazyLock::new(|| {
    Uuid::parse_str(SPACE_ID_PROPERTY_ID)
        .expect("SPACE_ID_PROPERTY_ID is a valid UUID constant")
});

static PROPOSAL_TYPE_UUID: LazyLock<Uuid> = LazyLock::new(|| {
    Uuid::parse_str(PROPOSAL_TYPE_ID).expect("PROPOSAL_TYPE_ID is a valid UUID constant")
});

pub struct SystemEntityResult {
    pub entities: Vec<EntityItem>,
    pub values: Vec<ValueOp>,
    pub relations: Vec<RelationOp>,
}

impl SystemEntityResult {
    pub fn values_to_set(&self) -> Vec<&ValueOp> {
        self.values
            .iter()
            .filter(|v| matches!(v.change_type, ValueChangeType::Set))
            .collect()
    }

    pub fn relations_to_create(&self) -> Vec<&SetRelationItem> {
        self.relations
            .iter()
            .filter_map(|op| match op {
                RelationOp::Create(r) => Some(r),
                _ => None,
            })
            .collect()
    }
}

fn derive_system_relation_id(type_name: &str, entity_id: &Uuid) -> Uuid {
    Uuid::new_v5(
        &GEO_SYSTEM_NS,
        format!("geo:system:rel:{}:{}", type_name, entity_id).as_bytes(),
    )
}

fn make_system_value_integer(
    entity_id: &Uuid,
    property_id: &Uuid,
    space_id: &Uuid,
    value: i64,
) -> ValueOp {
    ValueOp {
        id: derive_value_id(entity_id, property_id, space_id),
        change_type: ValueChangeType::Set,
        entity_id: *entity_id,
        property_id: *property_id,
        space_id: *space_id,
        integer: Some(value),
        language: None,
        unit: None,
        text: None,
        decimal: None,
        boolean: None,
        time: None,
        point: None,
        rect: None,
        float: None,
        bytes: None,
        date: None,
        datetime: None,
        schedule: None,
        embedding: None,
        time_utc: None,
        datetime_utc: None,
        context_root_id: None,
        context_edge_type_id: None,
    }
}

fn make_system_value_text(
    entity_id: &Uuid,
    property_id: &Uuid,
    space_id: &Uuid,
    value: &str,
) -> ValueOp {
    ValueOp {
        id: derive_value_id(entity_id, property_id, space_id),
        change_type: ValueChangeType::Set,
        entity_id: *entity_id,
        property_id: *property_id,
        space_id: *space_id,
        text: Some(value.to_string()),
        language: None,
        unit: None,
        bytes: None,
        decimal: None,
        boolean: None,
        time: None,
        point: None,
        rect: None,
        integer: None,
        float: None,
        date: None,
        datetime: None,
        schedule: None,
        embedding: None,
        time_utc: None,
        datetime_utc: None,
        context_root_id: None,
        context_edge_type_id: None,
    }
}

/// Derive the relation row ID (distinct from the reified entity ID).
fn derive_system_relation_row_id(type_name: &str, entity_id: &Uuid) -> Uuid {
    Uuid::new_v5(
        &GEO_SYSTEM_NS,
        format!("geo:system:rel_id:{}:{}", type_name, entity_id).as_bytes(),
    )
}

fn make_system_type_relation(
    entity_id: &Uuid,
    type_entity_id: &Uuid,
    type_name: &str,
    space_id: &Uuid,
) -> SetRelationItem {
    let reified_entity_id = derive_system_relation_id(type_name, entity_id);
    let relation_row_id = derive_system_relation_row_id(type_name, entity_id);
    SetRelationItem {
        id: relation_row_id,
        entity_id: reified_entity_id,
        type_id: *SYSTEM_TYPES_RELATION_TID,
        from_id: *entity_id,
        to_id: *type_entity_id,
        space_id: *space_id,
        from_space_id: None,
        from_version_id: None,
        to_space_id: None,
        to_version_id: None,
        position: None,
        verified: None,
        is_system: true,
        context_root_id: None,
        context_edge_type_id: None,
    }
}

/// Extract the space contract address bytes from a HermesCreateSpace message.
fn extract_space_address(space: &HermesCreateSpace) -> Result<Vec<u8>, HandlerError> {
    match &space.payload {
        Some(Payload::EoaSpace(eoa)) => Ok(eoa.owner.clone()),
        Some(Payload::DefaultDaoSpace(dao)) => Ok(dao.address.clone()),
        None => Err(HandlerError::MissingPayload),
    }
}

/// Map a SPACE_ID_REGISTERED event to system entity operations.
pub fn map_space_registered(
    space: &HermesCreateSpace,
    meta: &BlockchainMetadata,
) -> Result<SystemEntityResult, HandlerError> {
    let space_id = Uuid::from_slice(&space.space_id)?;
    let entity_id = space_id;

    let address = extract_space_address(space)?;
    let timestamp = meta.created_at.to_string();
    let block = meta.block_number.to_string();

    let entity = EntityItem {
        id: entity_id,
        created_at: timestamp.clone(),
        created_at_block: block.clone(),
        updated_at: timestamp,
        updated_at_block: block,
    };

    let values = vec![
        make_system_value_text(
            &entity_id,
            &SPACE_ADDRESS_PID,
            &space_id,
            &format!("0x{}", hex::encode(&address)),
        ),
        make_system_value_integer(
            &entity_id,
            &CREATED_AT_BLOCK_PID,
            &space_id,
            meta.block_number as i64,
        ),
        make_system_value_text(
            &entity_id,
            &NAME_PID,
            &space_id,
            &format!("Space {}", space_id),
        ),
        make_system_value_text(
            &entity_id,
            &DESCRIPTION_PID,
            &space_id,
            &format!("System entity for space {}", space_id),
        ),
    ];

    let mut relations = vec![
        RelationOp::Create(make_system_type_relation(
            &entity_id,
            &SYSTEM_TYPE_UUID,
            "System",
            &space_id,
        )),
        RelationOp::Create(make_system_type_relation(
            &entity_id,
            &SPACE_TYPE_UUID,
            "Space",
            &space_id,
        )),
    ];

    // Add SpaceType relation based on payload variant
    match &space.payload {
        Some(Payload::EoaSpace(_)) => {
            relations.push(RelationOp::Create(make_system_type_relation(
                &entity_id,
                &EOA_SPACE_TYPE_UUID,
                "EoaSpace",
                &space_id,
            )));
        }
        Some(Payload::DefaultDaoSpace(_)) => {
            relations.push(RelationOp::Create(make_system_type_relation(
                &entity_id,
                &DAO_SPACE_TYPE_UUID,
                "DaoSpace",
                &space_id,
            )));
        }
        None => {} // No SpaceType relation for unknown payloads
    }

    Ok(SystemEntityResult {
        entities: vec![entity],
        values,
        relations,
    })
}

/// Map a PROPOSAL_CREATED event to system entity operations.
pub fn map_proposal_created(
    msg: &HermesProposalCreated,
    meta: &BlockchainMetadata,
) -> Result<SystemEntityResult, HandlerError> {
    let space_id = Uuid::from_slice(&msg.space_id)?;
    let proposal_id = Uuid::from_slice(&msg.proposal_id)?;

    let entity_id = proposal_id;

    let timestamp = meta.created_at.to_string();
    let block = meta.block_number.to_string();

    let entity = EntityItem {
        id: entity_id,
        created_at: timestamp.clone(),
        created_at_block: block.clone(),
        updated_at: timestamp,
        updated_at_block: block,
    };

    // Map proto voting mode through enum for validation (consistent with governance.rs)
    let voting_mode_value = match ProtoVotingMode::try_from(msg.voting_mode) {
        Ok(ProtoVotingMode::Fast) => 0i64,
        Ok(ProtoVotingMode::Slow) | Err(_) => 1i64,
    };

    let values = vec![
        make_system_value_integer(&entity_id, &VOTING_MODE_PID, &space_id, voting_mode_value),
        make_system_value_text(
            &entity_id,
            &SPACE_ID_PROPERTY_PID,
            &space_id,
            &space_id.to_string(),
        ),
        make_system_value_integer(
            &entity_id,
            &CREATED_AT_BLOCK_PID,
            &space_id,
            meta.block_number as i64,
        ),
        make_system_value_text(
            &entity_id,
            &NAME_PID,
            &space_id,
            &format!("Proposal {}", proposal_id),
        ),
        make_system_value_text(
            &entity_id,
            &DESCRIPTION_PID,
            &space_id,
            &format!(
                "System entity for proposal {} in space {}",
                proposal_id, space_id
            ),
        ),
    ];

    let relations = vec![
        RelationOp::Create(make_system_type_relation(
            &entity_id,
            &SYSTEM_TYPE_UUID,
            "System",
            &space_id,
        )),
        RelationOp::Create(make_system_type_relation(
            &entity_id,
            &PROPOSAL_TYPE_UUID,
            "Proposal",
            &space_id,
        )),
    ];

    Ok(SystemEntityResult {
        entities: vec![entity],
        values,
        relations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_space(space_id_bytes: &[u8; 16], address: Vec<u8>) -> HermesCreateSpace {
        use hermes_schema::pb::space::DefaultDaoSpacePayload;
        HermesCreateSpace {
            space_id: space_id_bytes.to_vec(),
            meta: Some(BlockchainMetadata {
                created_at: 1000,
                created_by: vec![0xAA; 20],
                block_number: 42,
                cursor: String::new(),
                sequence: 0,
                is_last: true,
            }),
            payload: Some(Payload::DefaultDaoSpace(DefaultDaoSpacePayload { address })),
        }
    }

    fn make_eoa_test_space(space_id_bytes: &[u8; 16], owner: Vec<u8>) -> HermesCreateSpace {
        use hermes_schema::pb::space::EoaSpacePayload;
        HermesCreateSpace {
            space_id: space_id_bytes.to_vec(),
            meta: Some(BlockchainMetadata {
                created_at: 1000,
                created_by: vec![0xAA; 20],
                block_number: 42,
                cursor: String::new(),
                sequence: 0,
                is_last: true,
            }),
            payload: Some(Payload::EoaSpace(EoaSpacePayload { owner })),
        }
    }

    fn test_meta() -> BlockchainMetadata {
        BlockchainMetadata {
            created_at: 1000,
            created_by: vec![0xAA; 20],
            block_number: 42,
            cursor: String::new(),
            sequence: 0,
            is_last: true,
        }
    }

    #[test]
    fn space_entity_id_matches_onchain_space_id() {
        let space_id_bytes: [u8; 16] = [1; 16];
        let space = make_test_space(&space_id_bytes, vec![0xBB; 20]);
        let meta = test_meta();

        let result = map_space_registered(&space, &meta).unwrap();

        // Entity ID should be the onchain space_id directly
        let expected = Uuid::from_bytes(space_id_bytes);
        assert_eq!(result.entities[0].id, expected);
    }

    #[test]
    fn space_values_have_correct_properties_and_types() {
        let space_id_bytes: [u8; 16] = [2; 16];
        let address = vec![0xCC; 20];
        let space = make_test_space(&space_id_bytes, address.clone());
        let meta = test_meta();

        let result = map_space_registered(&space, &meta).unwrap();
        assert_eq!(result.values.len(), 4);

        let space_address_pid = Uuid::parse_str(SPACE_ADDRESS_PROPERTY_ID).unwrap();
        let created_at_block_pid = Uuid::parse_str(CREATED_AT_BLOCK_PROPERTY_ID).unwrap();

        // SpaceAddress — hex string
        let addr_val = result
            .values
            .iter()
            .find(|v| v.property_id == space_address_pid)
            .unwrap();
        assert_eq!(
            addr_val.text.as_ref().unwrap(),
            &format!("0x{}", hex::encode(&address)),
        );
        assert!(addr_val.bytes.is_none());
        assert!(addr_val.integer.is_none());

        // CreatedAtBlock — integer
        let block_val = result
            .values
            .iter()
            .find(|v| v.property_id == created_at_block_pid)
            .unwrap();
        assert_eq!(block_val.integer, Some(42));
        assert!(block_val.bytes.is_none());
    }

    #[test]
    fn space_has_system_and_space_type_relations() {
        let space_id_bytes: [u8; 16] = [3; 16];
        let space = make_test_space(&space_id_bytes, vec![0xDD; 20]);
        let meta = test_meta();

        let result = map_space_registered(&space, &meta).unwrap();
        let relations = result.relations_to_create();
        assert_eq!(relations.len(), 3);

        let system_types_tid = Uuid::parse_str(SYSTEM_TYPES_RELATION_TYPE_ID).unwrap();
        let system_type_eid = Uuid::parse_str(SYSTEM_TYPE_ID).unwrap();
        let space_type_eid = Uuid::parse_str(SPACE_TYPE_ID).unwrap();
        let dao_type_eid = Uuid::parse_str(DAO_SPACE_TYPE_ID).unwrap();

        // All use SYSTEM_TYPES_RELATION_TYPE_ID
        for rel in &relations {
            assert_eq!(rel.type_id, system_types_tid);
            assert_eq!(rel.from_id, result.entities[0].id);
        }

        let to_ids: Vec<Uuid> = relations.iter().map(|r| r.to_id).collect();
        assert!(to_ids.contains(&system_type_eid));
        assert!(to_ids.contains(&space_type_eid));
        assert!(to_ids.contains(&dao_type_eid));
    }

    #[test]
    fn relation_ids_dont_collide_with_entity_id() {
        let space_id_bytes: [u8; 16] = [4; 16];
        let space = make_test_space(&space_id_bytes, vec![0xEE; 20]);
        let meta = test_meta();

        let result = map_space_registered(&space, &meta).unwrap();
        let entity_id = result.entities[0].id;
        let relations = result.relations_to_create();

        for rel in &relations {
            assert_ne!(
                rel.id, entity_id,
                "Relation row ID should not collide with system entity ID"
            );
            assert_ne!(
                rel.entity_id, entity_id,
                "Relation entity ID should not collide with system entity ID"
            );
            assert_ne!(
                rel.id, rel.entity_id,
                "Relation row ID should differ from its reified entity ID"
            );
        }

        // Relation IDs should also be distinct from each other
        assert_ne!(relations[0].id, relations[1].id);
        assert_ne!(relations[0].entity_id, relations[1].entity_id);
    }

    #[test]
    fn missing_payload_returns_error() {
        let space = HermesCreateSpace {
            space_id: vec![5; 16],
            meta: Some(test_meta()),
            payload: None,
        };

        let result = map_space_registered(&space, &test_meta());
        assert!(result.is_err());
    }

    // ===================
    // Proposal mapping tests
    // ===================

    fn make_test_proposal(
        space_id: &[u8; 16],
        proposal_id: &[u8; 16],
        proposer_id: &[u8; 16],
        voting_mode: i32,
    ) -> HermesProposalCreated {
        HermesProposalCreated {
            space_id: space_id.to_vec(),
            proposer_id: proposer_id.to_vec(),
            proposal_id: proposal_id.to_vec(),
            voting_mode,
            actions: vec![],
            settings: None,
            meta: Some(test_meta()),
        }
    }

    #[test]
    fn space_name_and_description_contain_space_id() {
        let space_id_bytes: [u8; 16] = [6; 16];
        let space = make_test_space(&space_id_bytes, vec![0xFF; 20]);
        let meta = test_meta();

        let result = map_space_registered(&space, &meta).unwrap();
        let space_id = Uuid::from_bytes(space_id_bytes);

        let name_pid = Uuid::parse_str(NAME_PROPERTY_ID).unwrap();
        let desc_pid = Uuid::parse_str(DESCRIPTION_PROPERTY_ID).unwrap();

        let name_val = result
            .values
            .iter()
            .find(|v| v.property_id == name_pid)
            .unwrap();
        assert_eq!(
            name_val.text.as_ref().unwrap(),
            &format!("Space {}", space_id)
        );

        let desc_val = result
            .values
            .iter()
            .find(|v| v.property_id == desc_pid)
            .unwrap();
        assert_eq!(
            desc_val.text.as_ref().unwrap(),
            &format!("System entity for space {}", space_id),
        );
    }

    #[test]
    fn proposal_entity_id_matches_onchain_proposal_id() {
        let space_id: [u8; 16] = [10; 16];
        let proposal_id: [u8; 16] = [20; 16];
        let proposer_id: [u8; 16] = [30; 16];
        let msg = make_test_proposal(&space_id, &proposal_id, &proposer_id, 0);
        let meta = test_meta();

        let result = map_proposal_created(&msg, &meta).unwrap();

        // Entity ID should be the onchain proposal_id directly
        let expected = Uuid::from_bytes(proposal_id);
        assert_eq!(result.entities[0].id, expected);
    }

    #[test]
    fn proposal_values_have_correct_properties_and_types() {
        let space_id: [u8; 16] = [11; 16];
        let proposal_id: [u8; 16] = [21; 16];
        let proposer_id: [u8; 16] = [31; 16];
        let msg = make_test_proposal(&space_id, &proposal_id, &proposer_id, 1);
        let meta = test_meta();

        let result = map_proposal_created(&msg, &meta).unwrap();
        assert_eq!(result.values.len(), 5);

        let voting_mode_pid = Uuid::parse_str(VOTING_MODE_PROPERTY_ID).unwrap();
        let space_id_property_pid = Uuid::parse_str(SPACE_ID_PROPERTY_ID).unwrap();
        let created_at_block_pid = Uuid::parse_str(CREATED_AT_BLOCK_PROPERTY_ID).unwrap();

        // VotingMode — integer (Slow = 1)
        let vm_val = result
            .values
            .iter()
            .find(|v| v.property_id == voting_mode_pid)
            .unwrap();
        assert_eq!(vm_val.integer, Some(1));
        assert!(vm_val.bytes.is_none());

        // SpaceId — space id string
        let by_val = result
            .values
            .iter()
            .find(|v| v.property_id == space_id_property_pid)
            .unwrap();
        assert_eq!(
            by_val.text.as_ref().unwrap(),
            "0b0b0b0b-0b0b-0b0b-0b0b-0b0b0b0b0b0b",
        );
        assert!(by_val.bytes.is_none());

        // CreatedAtBlock — integer
        let block_val = result
            .values
            .iter()
            .find(|v| v.property_id == created_at_block_pid)
            .unwrap();
        assert_eq!(block_val.integer, Some(42));
    }

    #[test]
    fn proposal_has_system_and_proposal_type_relations() {
        let space_id: [u8; 16] = [12; 16];
        let proposal_id: [u8; 16] = [22; 16];
        let proposer_id: [u8; 16] = [32; 16];
        let msg = make_test_proposal(&space_id, &proposal_id, &proposer_id, 0);
        let meta = test_meta();

        let result = map_proposal_created(&msg, &meta).unwrap();
        let relations = result.relations_to_create();
        assert_eq!(relations.len(), 2);

        let system_types_tid = Uuid::parse_str(SYSTEM_TYPES_RELATION_TYPE_ID).unwrap();
        let system_type_eid = Uuid::parse_str(SYSTEM_TYPE_ID).unwrap();
        let proposal_type_eid = Uuid::parse_str(PROPOSAL_TYPE_ID).unwrap();

        for rel in &relations {
            assert_eq!(rel.type_id, system_types_tid);
            assert_eq!(rel.from_id, result.entities[0].id);
        }

        let to_ids: Vec<Uuid> = relations.iter().map(|r| r.to_id).collect();
        assert!(to_ids.contains(&system_type_eid));
        assert!(to_ids.contains(&proposal_type_eid));
    }

    #[test]
    fn proposal_name_and_description_contain_ids() {
        let space_id_bytes: [u8; 16] = [14; 16];
        let proposal_id_bytes: [u8; 16] = [24; 16];
        let proposer_id: [u8; 16] = [34; 16];
        let msg = make_test_proposal(&space_id_bytes, &proposal_id_bytes, &proposer_id, 0);
        let meta = test_meta();

        let result = map_proposal_created(&msg, &meta).unwrap();
        let space_id = Uuid::from_bytes(space_id_bytes);
        let proposal_id = Uuid::from_bytes(proposal_id_bytes);

        let name_pid = Uuid::parse_str(NAME_PROPERTY_ID).unwrap();
        let desc_pid = Uuid::parse_str(DESCRIPTION_PROPERTY_ID).unwrap();

        let name_val = result
            .values
            .iter()
            .find(|v| v.property_id == name_pid)
            .unwrap();
        assert_eq!(
            name_val.text.as_ref().unwrap(),
            &format!("Proposal {}", proposal_id)
        );

        let desc_val = result
            .values
            .iter()
            .find(|v| v.property_id == desc_pid)
            .unwrap();
        assert_eq!(
            desc_val.text.as_ref().unwrap(),
            &format!(
                "System entity for proposal {} in space {}",
                proposal_id, space_id
            ),
        );
    }

    #[test]
    fn proposal_entity_id_is_the_onchain_id_regardless_of_space() {
        let proposal_id: [u8; 16] = [99; 16];
        let proposer_id: [u8; 16] = [88; 16];
        let space_a: [u8; 16] = [1; 16];
        let space_b: [u8; 16] = [2; 16];

        let msg_a = make_test_proposal(&space_a, &proposal_id, &proposer_id, 0);
        let msg_b = make_test_proposal(&space_b, &proposal_id, &proposer_id, 0);
        let meta = test_meta();

        let result_a = map_proposal_created(&msg_a, &meta).unwrap();
        let result_b = map_proposal_created(&msg_b, &meta).unwrap();

        // Same proposal_id produces same entity ID (it's the onchain ID)
        let expected = Uuid::from_bytes(proposal_id);
        assert_eq!(result_a.entities[0].id, expected);
        assert_eq!(result_b.entities[0].id, expected);
    }

    #[test]
    fn proposal_voting_mode_fast_is_zero() {
        let msg = make_test_proposal(&[13; 16], &[23; 16], &[33; 16], 0); // Fast = 0
        let meta = test_meta();
        let result = map_proposal_created(&msg, &meta).unwrap();

        let voting_mode_pid = Uuid::parse_str(VOTING_MODE_PROPERTY_ID).unwrap();
        let vm_val = result
            .values
            .iter()
            .find(|v| v.property_id == voting_mode_pid)
            .unwrap();
        assert_eq!(vm_val.integer, Some(0));
    }

    // ===================
    // SpaceType relation tests
    // ===================

    #[test]
    fn dao_space_creates_three_relations() {
        let space_id_bytes: [u8; 16] = [41; 16];
        let space = make_test_space(&space_id_bytes, vec![0xCC; 20]);
        let meta = test_meta();

        let result = map_space_registered(&space, &meta).unwrap();
        let relations = result.relations_to_create();
        assert_eq!(relations.len(), 3);

        let dao_type_uuid = Uuid::parse_str(DAO_SPACE_TYPE_ID).unwrap();
        let to_ids: Vec<Uuid> = relations.iter().map(|r| r.to_id).collect();
        assert!(
            to_ids.contains(&dao_type_uuid),
            "Should contain DAO_SPACE_TYPE_ID relation"
        );
    }

    #[test]
    fn eoa_space_creates_three_relations() {
        let space_id_bytes: [u8; 16] = [40; 16];
        let space = make_eoa_test_space(&space_id_bytes, vec![0xBB; 20]);
        let meta = test_meta();

        let result = map_space_registered(&space, &meta).unwrap();
        let relations = result.relations_to_create();
        assert_eq!(relations.len(), 3);

        let eoa_type_uuid = Uuid::parse_str(EOA_SPACE_TYPE_ID).unwrap();
        let to_ids: Vec<Uuid> = relations.iter().map(|r| r.to_id).collect();
        assert!(
            to_ids.contains(&eoa_type_uuid),
            "Should contain EOA_SPACE_TYPE_ID relation"
        );
    }

    #[test]
    fn space_type_relation_ids_are_deterministic() {
        let space_id_bytes: [u8; 16] = [42; 16];
        let space = make_eoa_test_space(&space_id_bytes, vec![0xDD; 20]);
        let meta = test_meta();

        let result1 = map_space_registered(&space, &meta).unwrap();
        let result2 = map_space_registered(&space, &meta).unwrap();

        let rels1 = result1.relations_to_create();
        let rels2 = result2.relations_to_create();

        assert_eq!(rels1.len(), rels2.len());
        for (r1, r2) in rels1.iter().zip(rels2.iter()) {
            assert_eq!(r1.id, r2.id, "Relation row IDs should be deterministic");
            assert_eq!(
                r1.entity_id, r2.entity_id,
                "Reified entity IDs should be deterministic"
            );
        }
    }

    #[test]
    fn space_type_relation_ids_differ_between_eoa_and_dao() {
        let space_id_bytes: [u8; 16] = [43; 16];
        let eoa_space = make_eoa_test_space(&space_id_bytes, vec![0xEE; 20]);
        let dao_space = make_test_space(&space_id_bytes, vec![0xEE; 20]);
        let meta = test_meta();

        let eoa_result = map_space_registered(&eoa_space, &meta).unwrap();
        let dao_result = map_space_registered(&dao_space, &meta).unwrap();

        let eoa_rels = eoa_result.relations_to_create();
        let dao_rels = dao_result.relations_to_create();

        // Find SpaceType relations by to_id rather than assuming index
        let eoa_type_uuid = Uuid::parse_str(EOA_SPACE_TYPE_ID).unwrap();
        let dao_type_uuid = Uuid::parse_str(DAO_SPACE_TYPE_ID).unwrap();
        let eoa_space_type_rel = eoa_rels.iter().find(|r| r.to_id == eoa_type_uuid).unwrap();
        let dao_space_type_rel = dao_rels.iter().find(|r| r.to_id == dao_type_uuid).unwrap();

        assert_ne!(
            eoa_space_type_rel.entity_id, dao_space_type_rel.entity_id,
            "EOA and DAO should have different reified entity IDs"
        );
        assert_ne!(
            eoa_space_type_rel.id, dao_space_type_rel.id,
            "EOA and DAO should have different relation row IDs"
        );
        assert_ne!(
            eoa_space_type_rel.to_id, dao_space_type_rel.to_id,
            "EOA and DAO should point to different type entities"
        );
    }
}
