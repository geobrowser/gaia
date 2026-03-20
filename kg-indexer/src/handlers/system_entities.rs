use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;
use hermes_schema::pb::governance::{HermesProposalCreated, VotingMode as ProtoVotingMode};
use hermes_schema::pb::space::{hermes_create_space::Payload, HermesCreateSpace};
use sdk::core::ids::{
    CREATED_AT_BLOCK_PROPERTY_ID, CREATED_BY_PROPERTY_ID, GEO_SYSTEM_NAMESPACE,
    PROPOSAL_ID_PROPERTY_ID, PROPOSAL_TYPE_ID, SPACE_ADDRESS_PROPERTY_ID, SPACE_TYPE_ID,
    SYSTEM_TYPES_RELATION_TYPE_ID, SYSTEM_TYPE_ID, VOTING_MODE_PROPERTY_ID,
};
use uuid::Uuid;

use crate::error::HandlerError;
use crate::handlers::edits::derive_value_id;
use crate::models::{
    entities::EntityItem,
    relations::{RelationOp, SetRelationItem},
    values::{ValueChangeType, ValueOp},
};

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

fn geo_system_namespace() -> Uuid {
    Uuid::parse_str(GEO_SYSTEM_NAMESPACE).expect("GEO_SYSTEM_NAMESPACE is a valid UUID constant")
}

fn derive_system_entity_id(event_type: &str, unique_data: &str) -> Uuid {
    Uuid::new_v5(
        &geo_system_namespace(),
        format!("geo:system:{}:{}", event_type, unique_data).as_bytes(),
    )
}

fn derive_system_relation_id(type_name: &str, entity_id: &Uuid) -> Uuid {
    Uuid::new_v5(
        &geo_system_namespace(),
        format!("geo:system:rel:{}:{}", type_name, entity_id).as_bytes(),
    )
}

fn make_system_value_bytes(
    entity_id: &Uuid,
    property_id: &Uuid,
    space_id: &Uuid,
    data: &[u8],
) -> ValueOp {
    ValueOp {
        id: derive_value_id(entity_id, property_id, space_id),
        change_type: ValueChangeType::Set,
        entity_id: *entity_id,
        property_id: *property_id,
        space_id: *space_id,
        bytes: Some(data.to_vec()),
        language: None,
        unit: None,
        text: None,
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

/// Derive the relation row ID (distinct from the reified entity ID).
fn derive_system_relation_row_id(type_name: &str, entity_id: &Uuid) -> Uuid {
    Uuid::new_v5(
        &geo_system_namespace(),
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
        type_id: Uuid::parse_str(SYSTEM_TYPES_RELATION_TYPE_ID)
            .expect("SYSTEM_TYPES_RELATION_TYPE_ID is a valid UUID constant"),
        from_id: *entity_id,
        to_id: *type_entity_id,
        space_id: *space_id,
        from_space_id: None,
        from_version_id: None,
        to_space_id: None,
        to_version_id: None,
        position: None,
        verified: None,
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
    let entity_id =
        derive_system_entity_id("GOVERNANCE.SPACE_ID_REGISTERED", &space_id.to_string());

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

    let space_address_pid =
        Uuid::parse_str(SPACE_ADDRESS_PROPERTY_ID).expect("SPACE_ADDRESS_PROPERTY_ID is a valid UUID constant");
    let created_by_pid =
        Uuid::parse_str(CREATED_BY_PROPERTY_ID).expect("CREATED_BY_PROPERTY_ID is a valid UUID constant");
    let created_at_block_pid =
        Uuid::parse_str(CREATED_AT_BLOCK_PROPERTY_ID).expect("CREATED_AT_BLOCK_PROPERTY_ID is a valid UUID constant");

    let values = vec![
        make_system_value_bytes(&entity_id, &space_address_pid, &space_id, &address),
        make_system_value_bytes(&entity_id, &created_by_pid, &space_id, space_id.as_bytes()),
        make_system_value_integer(
            &entity_id,
            &created_at_block_pid,
            &space_id,
            meta.block_number as i64,
        ),
    ];

    let system_type_uuid =
        Uuid::parse_str(SYSTEM_TYPE_ID).expect("SYSTEM_TYPE_ID is a valid UUID constant");
    let space_type_uuid =
        Uuid::parse_str(SPACE_TYPE_ID).expect("SPACE_TYPE_ID is a valid UUID constant");

    let relations = vec![
        RelationOp::Create(make_system_type_relation(
            &entity_id,
            &system_type_uuid,
            "System",
            &space_id,
        )),
        RelationOp::Create(make_system_type_relation(
            &entity_id,
            &space_type_uuid,
            "Space",
            &space_id,
        )),
    ];

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

    let entity_id = derive_system_entity_id(
        "GOVERNANCE.PROPOSAL_CREATED",
        &format!("{}:{}", space_id, proposal_id),
    );

    let timestamp = meta.created_at.to_string();
    let block = meta.block_number.to_string();

    let entity = EntityItem {
        id: entity_id,
        created_at: timestamp.clone(),
        created_at_block: block.clone(),
        updated_at: timestamp,
        updated_at_block: block,
    };

    let proposal_id_pid = Uuid::parse_str(PROPOSAL_ID_PROPERTY_ID)
        .expect("PROPOSAL_ID_PROPERTY_ID is a valid UUID constant");
    let voting_mode_pid = Uuid::parse_str(VOTING_MODE_PROPERTY_ID)
        .expect("VOTING_MODE_PROPERTY_ID is a valid UUID constant");
    let created_by_pid =
        Uuid::parse_str(CREATED_BY_PROPERTY_ID).expect("CREATED_BY_PROPERTY_ID is a valid UUID constant");
    let created_at_block_pid = Uuid::parse_str(CREATED_AT_BLOCK_PROPERTY_ID)
        .expect("CREATED_AT_BLOCK_PROPERTY_ID is a valid UUID constant");

    // Map proto voting mode through enum for validation (consistent with governance.rs)
    let voting_mode_value = match ProtoVotingMode::try_from(msg.voting_mode) {
        Ok(ProtoVotingMode::Fast) => 0i64,
        Ok(ProtoVotingMode::Slow) | Err(_) => 1i64,
    };

    let values = vec![
        make_system_value_bytes(&entity_id, &proposal_id_pid, &space_id, &msg.proposal_id),
        make_system_value_integer(
            &entity_id,
            &voting_mode_pid,
            &space_id,
            voting_mode_value,
        ),
        make_system_value_bytes(&entity_id, &created_by_pid, &space_id, &msg.proposer_id),
        make_system_value_integer(
            &entity_id,
            &created_at_block_pid,
            &space_id,
            meta.block_number as i64,
        ),
    ];

    let system_type_uuid =
        Uuid::parse_str(SYSTEM_TYPE_ID).expect("SYSTEM_TYPE_ID is a valid UUID constant");
    let proposal_type_uuid =
        Uuid::parse_str(PROPOSAL_TYPE_ID).expect("PROPOSAL_TYPE_ID is a valid UUID constant");

    let relations = vec![
        RelationOp::Create(make_system_type_relation(
            &entity_id,
            &system_type_uuid,
            "System",
            &space_id,
        )),
        RelationOp::Create(make_system_type_relation(
            &entity_id,
            &proposal_type_uuid,
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
    fn space_entity_id_is_deterministic() {
        let space_id_bytes: [u8; 16] = [1; 16];
        let space = make_test_space(&space_id_bytes, vec![0xBB; 20]);
        let meta = test_meta();

        let result1 = map_space_registered(&space, &meta).unwrap();
        let result2 = map_space_registered(&space, &meta).unwrap();

        assert_eq!(result1.entities[0].id, result2.entities[0].id);

        // Verify it's a proper UUID v5 derivation
        let space_id = Uuid::from_bytes(space_id_bytes);
        let expected =
            derive_system_entity_id("GOVERNANCE.SPACE_ID_REGISTERED", &space_id.to_string());
        assert_eq!(result1.entities[0].id, expected);
    }

    #[test]
    fn space_values_have_correct_properties_and_types() {
        let space_id_bytes: [u8; 16] = [2; 16];
        let address = vec![0xCC; 20];
        let space = make_test_space(&space_id_bytes, address.clone());
        let meta = test_meta();

        let result = map_space_registered(&space, &meta).unwrap();
        assert_eq!(result.values.len(), 3);

        let space_address_pid = Uuid::parse_str(SPACE_ADDRESS_PROPERTY_ID).unwrap();
        let created_by_pid = Uuid::parse_str(CREATED_BY_PROPERTY_ID).unwrap();
        let created_at_block_pid = Uuid::parse_str(CREATED_AT_BLOCK_PROPERTY_ID).unwrap();

        // SpaceAddress — bytes
        let addr_val = result
            .values
            .iter()
            .find(|v| v.property_id == space_address_pid)
            .unwrap();
        assert_eq!(addr_val.bytes.as_ref().unwrap(), &address);
        assert!(addr_val.integer.is_none());

        // CreatedBy — bytes
        let by_val = result
            .values
            .iter()
            .find(|v| v.property_id == created_by_pid)
            .unwrap();
        assert_eq!(by_val.bytes.as_ref().unwrap(), &space_id_bytes.to_vec());

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
        assert_eq!(relations.len(), 2);

        let system_types_tid = Uuid::parse_str(SYSTEM_TYPES_RELATION_TYPE_ID).unwrap();
        let system_type_eid = Uuid::parse_str(SYSTEM_TYPE_ID).unwrap();
        let space_type_eid = Uuid::parse_str(SPACE_TYPE_ID).unwrap();

        // Both use SYSTEM_TYPES_RELATION_TYPE_ID
        for rel in &relations {
            assert_eq!(rel.type_id, system_types_tid);
            assert_eq!(rel.from_id, result.entities[0].id);
        }

        let to_ids: Vec<Uuid> = relations.iter().map(|r| r.to_id).collect();
        assert!(to_ids.contains(&system_type_eid));
        assert!(to_ids.contains(&space_type_eid));
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
    fn proposal_entity_id_is_deterministic() {
        let space_id: [u8; 16] = [10; 16];
        let proposal_id: [u8; 16] = [20; 16];
        let proposer_id: [u8; 16] = [30; 16];
        let msg = make_test_proposal(&space_id, &proposal_id, &proposer_id, 0);
        let meta = test_meta();

        let result1 = map_proposal_created(&msg, &meta).unwrap();
        let result2 = map_proposal_created(&msg, &meta).unwrap();
        assert_eq!(result1.entities[0].id, result2.entities[0].id);

        // Verify derivation uses "{space_id}:{proposal_id}"
        let sid = Uuid::from_bytes(space_id);
        let pid = Uuid::from_bytes(proposal_id);
        let expected = derive_system_entity_id(
            "GOVERNANCE.PROPOSAL_CREATED",
            &format!("{}:{}", sid, pid),
        );
        assert_eq!(result1.entities[0].id, expected);
    }

    #[test]
    fn proposal_values_have_correct_properties_and_types() {
        let space_id: [u8; 16] = [11; 16];
        let proposal_id: [u8; 16] = [21; 16];
        let proposer_id: [u8; 16] = [31; 16];
        let msg = make_test_proposal(&space_id, &proposal_id, &proposer_id, 1);
        let meta = test_meta();

        let result = map_proposal_created(&msg, &meta).unwrap();
        assert_eq!(result.values.len(), 4);

        let proposal_id_pid = Uuid::parse_str(PROPOSAL_ID_PROPERTY_ID).unwrap();
        let voting_mode_pid = Uuid::parse_str(VOTING_MODE_PROPERTY_ID).unwrap();
        let created_by_pid = Uuid::parse_str(CREATED_BY_PROPERTY_ID).unwrap();
        let created_at_block_pid = Uuid::parse_str(CREATED_AT_BLOCK_PROPERTY_ID).unwrap();

        // ProposalId — bytes
        let pid_val = result.values.iter().find(|v| v.property_id == proposal_id_pid).unwrap();
        assert_eq!(pid_val.bytes.as_ref().unwrap(), &proposal_id.to_vec());

        // VotingMode — integer (Slow = 1)
        let vm_val = result.values.iter().find(|v| v.property_id == voting_mode_pid).unwrap();
        assert_eq!(vm_val.integer, Some(1));
        assert!(vm_val.bytes.is_none());

        // CreatedBy — bytes (proposer space_id)
        let by_val = result.values.iter().find(|v| v.property_id == created_by_pid).unwrap();
        assert_eq!(by_val.bytes.as_ref().unwrap(), &proposer_id.to_vec());

        // CreatedAtBlock — integer
        let block_val = result.values.iter().find(|v| v.property_id == created_at_block_pid).unwrap();
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
    fn proposal_cross_space_uniqueness() {
        let proposal_id: [u8; 16] = [99; 16];
        let proposer_id: [u8; 16] = [88; 16];
        let space_a: [u8; 16] = [1; 16];
        let space_b: [u8; 16] = [2; 16];

        let msg_a = make_test_proposal(&space_a, &proposal_id, &proposer_id, 0);
        let msg_b = make_test_proposal(&space_b, &proposal_id, &proposer_id, 0);
        let meta = test_meta();

        let result_a = map_proposal_created(&msg_a, &meta).unwrap();
        let result_b = map_proposal_created(&msg_b, &meta).unwrap();

        assert_ne!(
            result_a.entities[0].id, result_b.entities[0].id,
            "Same proposal_id in different spaces must produce different entity IDs"
        );
    }

    #[test]
    fn proposal_voting_mode_fast_is_zero() {
        let msg = make_test_proposal(&[13; 16], &[23; 16], &[33; 16], 0); // Fast = 0
        let meta = test_meta();
        let result = map_proposal_created(&msg, &meta).unwrap();

        let voting_mode_pid = Uuid::parse_str(VOTING_MODE_PROPERTY_ID).unwrap();
        let vm_val = result.values.iter().find(|v| v.property_id == voting_mode_pid).unwrap();
        assert_eq!(vm_val.integer, Some(0));
    }
}
