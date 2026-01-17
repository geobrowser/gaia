use anyhow::Result;
use prost::Message;
use uuid::Uuid;

use hermes_schema::pb::knowledge::HermesEdit;
use wire::pb::grc20::{Op, Relation};

use sdk::core::ids::TYPE_RELATION_TYPE_ID;

/// Generate a CreateRelation operation for a type relation
pub fn create_type_relation(
    edit_name: &str,
    space_id: Uuid,
    entity_id: Uuid,
    type_entity_id: Uuid,
) -> Result<Vec<u8>> {
    create_type_relation_with_id(edit_name, space_id, Uuid::new_v4(), entity_id, type_entity_id)
}

/// Generate a CreateRelation operation for a type relation with a specific relation ID
pub fn create_type_relation_with_id(
    edit_name: &str,
    space_id: Uuid,
    relation_id: Uuid,
    entity_id: Uuid,
    type_entity_id: Uuid,
) -> Result<Vec<u8>> {
    let relation = Relation {
        id: relation_id.as_bytes().to_vec(),
        r#type: Uuid::parse_str(TYPE_RELATION_TYPE_ID)?.as_bytes().to_vec(),
        from_entity: entity_id.as_bytes().to_vec(),
        from_space: None,
        from_version: None,
        to_entity: type_entity_id.as_bytes().to_vec(),
        to_space: None,
        to_version: None,
        entity: entity_id.as_bytes().to_vec(),
        position: None,
        verified: None,
    };

    let op = Op {
        payload: Some(wire::pb::grc20::op::Payload::CreateRelation(relation)),
    };

    let edit = HermesEdit {
        id: Uuid::new_v4().as_bytes().to_vec(),
        name: edit_name.to_string(),
        ops: vec![op],
        authors: vec![Uuid::new_v4().as_bytes().to_vec()],
        language: None,
        space_id: space_id.as_bytes().to_vec(),
        is_canonical: true,
        meta: None,
    };

    let mut buf = Vec::new();
    edit.encode(&mut buf)?;
    Ok(buf)
}

/// Generate a CreateRelation operation for a custom relation type
#[allow(dead_code)]
pub fn create_custom_relation(
    edit_name: &str,
    space_id: Uuid,
    relation_id: Uuid,
    relation_type_id: Uuid,
    from_entity_id: Uuid,
    to_entity_id: Uuid,
) -> Result<Vec<u8>> {
    let relation = Relation {
        id: relation_id.as_bytes().to_vec(),
        r#type: relation_type_id.as_bytes().to_vec(),
        from_entity: from_entity_id.as_bytes().to_vec(),
        from_space: None,
        from_version: None,
        to_entity: to_entity_id.as_bytes().to_vec(),
        to_space: None,
        to_version: None,
        entity: from_entity_id.as_bytes().to_vec(),
        position: None,
        verified: None,
    };

    let op = Op {
        payload: Some(wire::pb::grc20::op::Payload::CreateRelation(relation)),
    };

    let edit = HermesEdit {
        id: Uuid::new_v4().as_bytes().to_vec(),
        name: edit_name.to_string(),
        ops: vec![op],
        authors: vec![Uuid::new_v4().as_bytes().to_vec()],
        language: None,
        space_id: space_id.as_bytes().to_vec(),
        is_canonical: true,
        meta: None,
    };

    let mut buf = Vec::new();
    edit.encode(&mut buf)?;
    Ok(buf)
}

/// Generate a DeleteRelation operation
pub fn delete_relation(
    edit_name: &str,
    space_id: Uuid,
    relation_id: Uuid,
) -> Result<Vec<u8>> {
    let op = Op {
        payload: Some(wire::pb::grc20::op::Payload::DeleteRelation(
            relation_id.as_bytes().to_vec(),
        )),
    };

    let edit = HermesEdit {
        id: Uuid::new_v4().as_bytes().to_vec(),
        name: edit_name.to_string(),
        ops: vec![op],
        authors: vec![Uuid::new_v4().as_bytes().to_vec()],
        language: None,
        space_id: space_id.as_bytes().to_vec(),
        is_canonical: true,
        meta: None,
    };

    let mut buf = Vec::new();
    edit.encode(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_type_relation() {
        let space_id = Uuid::new_v4();
        let entity_id = Uuid::new_v4();
        let type_entity_id = Uuid::new_v4();

        let result = create_type_relation("Create Type Relation", space_id, entity_id, type_entity_id);

        assert!(result.is_ok());
        let bytes = result.unwrap();
        assert!(!bytes.is_empty());

        // Verify we can decode it
        let decoded = HermesEdit::decode(&bytes[..]);
        assert!(decoded.is_ok());
        let edit = decoded.unwrap();
        assert_eq!(edit.name, "Create Type Relation");
        assert_eq!(edit.ops.len(), 1);
    }

    #[test]
    fn test_delete_relation() {
        let space_id = Uuid::new_v4();
        let relation_id = Uuid::new_v4();

        let result = delete_relation("Delete Relation", space_id, relation_id);

        assert!(result.is_ok());
    }
}
