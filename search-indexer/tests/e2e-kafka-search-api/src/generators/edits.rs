use anyhow::Result;
use prost::Message;
use uuid::Uuid;

use hermes_schema::pb::knowledge::HermesEdit;
use wire::pb::grc20::{Entity, Op, Value};

use sdk::core::ids::{AVATAR_PROPERTY_ID, DESCRIPTION_PROPERTY_ID, NAME_PROPERTY_ID};

/// Generate an UpdateEntity operation with name and description
pub fn create_entity_edit(
    edit_name: &str,
    space_id: Uuid,
    entity_id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
    avatar: Option<&str>,
) -> Result<Vec<u8>> {
    let mut values = Vec::new();

    // Add name if provided
    if let Some(name_value) = name {
        values.push(Value {
            property: Uuid::parse_str(NAME_PROPERTY_ID)?.as_bytes().to_vec(),
            value: name_value.to_string(),
            options: None,
        });
    }

    // Add description if provided
    if let Some(desc_value) = description {
        values.push(Value {
            property: Uuid::parse_str(DESCRIPTION_PROPERTY_ID)?
                .as_bytes()
                .to_vec(),
            value: desc_value.to_string(),
            options: None,
        });
    }

    // Add avatar if provided
    if let Some(avatar_value) = avatar {
        values.push(Value {
            property: Uuid::parse_str(AVATAR_PROPERTY_ID)?.as_bytes().to_vec(),
            value: avatar_value.to_string(),
            options: None,
        });
    }

    let entity = Entity {
        id: entity_id.as_bytes().to_vec(),
        values,
    };

    let op = Op {
        payload: Some(wire::pb::grc20::op::Payload::UpdateEntity(entity)),
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

/// Generate an edit that unsets entity properties
#[allow(dead_code)]
pub fn unset_entity_properties(
    edit_name: &str,
    space_id: Uuid,
    entity_id: Uuid,
    property_ids: Vec<&str>,
) -> Result<Vec<u8>> {
    let properties: Result<Vec<_>> = property_ids
        .into_iter()
        .map(|id| Ok(Uuid::parse_str(id)?.as_bytes().to_vec()))
        .collect();

    let unset_values = wire::pb::grc20::UnsetEntityValues {
        id: entity_id.as_bytes().to_vec(),
        properties: properties?,
    };

    let op = Op {
        payload: Some(wire::pb::grc20::op::Payload::UnsetEntityValues(unset_values)),
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

/// Generate a DeleteEntity operation
pub fn delete_entity(
    edit_name: &str,
    space_id: Uuid,
    entity_id: Uuid,
) -> Result<Vec<u8>> {
    let op = Op {
        payload: Some(wire::pb::grc20::op::Payload::DeleteEntity(
            entity_id.as_bytes().to_vec(),
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
    fn test_create_entity_edit() {
        let space_id = Uuid::new_v4();
        let entity_id = Uuid::new_v4();

        let result = create_entity_edit(
            "Test Edit",
            space_id,
            entity_id,
            Some("Test Entity"),
            Some("A test description"),
            None,
        );

        assert!(result.is_ok());
        let bytes = result.unwrap();
        assert!(!bytes.is_empty());

        // Verify we can decode it
        let decoded = HermesEdit::decode(&bytes[..]);
        assert!(decoded.is_ok());
        let edit = decoded.unwrap();
        assert_eq!(edit.name, "Test Edit");
        assert_eq!(edit.ops.len(), 1);
    }

    #[test]
    fn test_unset_entity_properties() {
        let space_id = Uuid::new_v4();
        let entity_id = Uuid::new_v4();

        let result = unset_entity_properties(
            "Unset Properties",
            space_id,
            entity_id,
            vec![NAME_PROPERTY_ID, DESCRIPTION_PROPERTY_ID],
        );

        assert!(result.is_ok());
    }
}
