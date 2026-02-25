use anyhow::Result;
use prost::Message;
use uuid::Uuid;

use grc_20::{encode_edit, CreateRelation, DeleteRelation, Edit as Grc20Edit, Op as Grc20Op};
use hermes_schema::pb::knowledge::HermesEdit;

use sdk::core::ids::{AVATAR_RELATION_TYPE_ID, COVER_RELATION_TYPE_ID, TYPE_RELATION_TYPE_ID};

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
    create_custom_relation(
        edit_name,
        space_id,
        relation_id,
        Uuid::parse_str(TYPE_RELATION_TYPE_ID)?,
        entity_id,
        type_entity_id,
    )
}

/// Generate a CreateRelation operation for a custom relation type
pub fn create_custom_relation(
    edit_name: &str,
    space_id: Uuid,
    relation_id: Uuid,
    relation_type_id: Uuid,
    from_entity_id: Uuid,
    to_entity_id: Uuid,
) -> Result<Vec<u8>> {
    let relation = CreateRelation {
        id: *relation_id.as_bytes(),
        entity: Some(*from_entity_id.as_bytes()),
        relation_type: *relation_type_id.as_bytes(),
        from: *from_entity_id.as_bytes(),
        from_is_value_ref: false,
        from_space: None,
        from_version: None,
        to: *to_entity_id.as_bytes(),
        to_is_value_ref: false,
        to_space: None,
        to_version: None,
        position: None,
        context: None,
    };

    let grc20_edit = Grc20Edit {
        id: *Uuid::new_v4().as_bytes(),
        name: edit_name.into(),
        authors: vec![*Uuid::new_v4().as_bytes()],
        created_at: 0,
        ops: vec![Grc20Op::CreateRelation(relation)],
    };

    let payload = encode_edit(&grc20_edit)?;

    let edit = HermesEdit {
        id: grc20_edit.id.to_vec(),
        name: edit_name.to_string(),
        payload,
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

/// Generate a CreateRelation operation for an avatar relation
pub fn create_avatar_relation(
    edit_name: &str,
    space_id: Uuid,
    relation_id: Uuid,
    entity_id: Uuid,
    image_entity_id: Uuid,
) -> Result<Vec<u8>> {
    create_custom_relation(
        edit_name,
        space_id,
        relation_id,
        Uuid::parse_str(AVATAR_RELATION_TYPE_ID)?,
        entity_id,
        image_entity_id,
    )
}

/// Generate a CreateRelation operation for a cover relation
pub fn create_cover_relation(
    edit_name: &str,
    space_id: Uuid,
    relation_id: Uuid,
    entity_id: Uuid,
    image_entity_id: Uuid,
) -> Result<Vec<u8>> {
    create_custom_relation(
        edit_name,
        space_id,
        relation_id,
        Uuid::parse_str(COVER_RELATION_TYPE_ID)?,
        entity_id,
        image_entity_id,
    )
}

/// Generate a DeleteRelation operation
pub fn delete_relation(edit_name: &str, space_id: Uuid, relation_id: Uuid) -> Result<Vec<u8>> {
    let delete = DeleteRelation {
        id: *relation_id.as_bytes(),
        context: None,
    };

    let grc20_edit = Grc20Edit {
        id: *Uuid::new_v4().as_bytes(),
        name: edit_name.into(),
        authors: vec![*Uuid::new_v4().as_bytes()],
        created_at: 0,
        ops: vec![Grc20Op::DeleteRelation(delete)],
    };

    let payload = encode_edit(&grc20_edit)?;

    let edit = HermesEdit {
        id: grc20_edit.id.to_vec(),
        name: edit_name.to_string(),
        payload,
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
