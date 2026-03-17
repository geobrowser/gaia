use anyhow::Result;
use prost::Message;
use uuid::Uuid;

use grc_20::model::RestoreEntity;
use grc_20::{
    encode_edit, CreateEntity, DeleteEntity, Edit as Grc20Edit, Op as Grc20Op, PropertyValue,
    UnsetLanguage, UnsetValue, UpdateEntity, Value as Grc20Value,
};
use hermes_schema::pb::knowledge::HermesEdit;

use sdk::core::ids::{DESCRIPTION_PROPERTY_ID, IMAGE_URL_PROPERTY_ID, NAME_PROPERTY_ID};

/// Generate an UpdateEntity operation with name, description, and optional image_url
pub fn create_entity_edit(
    edit_name: &str,
    space_id: Uuid,
    entity_id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
    image_url: Option<&str>,
) -> Result<Vec<u8>> {
    let mut set_properties = Vec::new();

    if let Some(name_value) = name {
        set_properties.push(PropertyValue {
            property: *Uuid::parse_str(NAME_PROPERTY_ID)?.as_bytes(),
            value: Grc20Value::Text {
                value: name_value.into(),
                language: None,
            },
        });
    }

    if let Some(desc_value) = description {
        set_properties.push(PropertyValue {
            property: *Uuid::parse_str(DESCRIPTION_PROPERTY_ID)?.as_bytes(),
            value: Grc20Value::Text {
                value: desc_value.into(),
                language: None,
            },
        });
    }

    if let Some(image_url_value) = image_url {
        set_properties.push(PropertyValue {
            property: *Uuid::parse_str(IMAGE_URL_PROPERTY_ID)?.as_bytes(),
            value: Grc20Value::Text {
                value: image_url_value.into(),
                language: None,
            },
        });
    }

    let update_entity = UpdateEntity {
        id: *entity_id.as_bytes(),
        set_properties,
        unset_values: vec![],
        context: None,
    };

    let grc20_edit = Grc20Edit {
        id: *Uuid::new_v4().as_bytes(),
        name: edit_name.into(),
        authors: vec![*Uuid::new_v4().as_bytes()],
        created_at: 0,
        ops: vec![Grc20Op::UpdateEntity(update_entity)],
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

/// Generate an edit that unsets entity properties
pub fn unset_entity_properties(
    edit_name: &str,
    space_id: Uuid,
    entity_id: Uuid,
    property_ids: Vec<&str>,
) -> Result<Vec<u8>> {
    let unset_values: Result<Vec<_>> = property_ids
        .into_iter()
        .map(|id| {
            Ok(UnsetValue {
                property: *Uuid::parse_str(id)?.as_bytes(),
                language: UnsetLanguage::All,
            })
        })
        .collect();

    let update_entity = UpdateEntity {
        id: *entity_id.as_bytes(),
        set_properties: vec![],
        unset_values: unset_values?,
        context: None,
    };

    let grc20_edit = Grc20Edit {
        id: *Uuid::new_v4().as_bytes(),
        name: edit_name.into(),
        authors: vec![*Uuid::new_v4().as_bytes()],
        created_at: 0,
        ops: vec![Grc20Op::UpdateEntity(update_entity)],
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

/// Generate an edit that both sets and unsets properties (tests LWW semantics)
pub fn update_entity_with_set_and_unset(
    edit_name: &str,
    space_id: Uuid,
    entity_id: Uuid,
    set_name: Option<&str>,
    set_description: Option<&str>,
    set_image_url: Option<&str>,
    unset_property_ids: Vec<&str>,
) -> Result<Vec<u8>> {
    let mut set_properties = Vec::new();

    if let Some(name_value) = set_name {
        set_properties.push(PropertyValue {
            property: *Uuid::parse_str(NAME_PROPERTY_ID)?.as_bytes(),
            value: Grc20Value::Text {
                value: name_value.into(),
                language: None,
            },
        });
    }

    if let Some(desc_value) = set_description {
        set_properties.push(PropertyValue {
            property: *Uuid::parse_str(DESCRIPTION_PROPERTY_ID)?.as_bytes(),
            value: Grc20Value::Text {
                value: desc_value.into(),
                language: None,
            },
        });
    }

    if let Some(image_url_value) = set_image_url {
        set_properties.push(PropertyValue {
            property: *Uuid::parse_str(IMAGE_URL_PROPERTY_ID)?.as_bytes(),
            value: Grc20Value::Text {
                value: image_url_value.into(),
                language: None,
            },
        });
    }

    let unset_values: Result<Vec<_>> = unset_property_ids
        .into_iter()
        .map(|id| {
            Ok(UnsetValue {
                property: *Uuid::parse_str(id)?.as_bytes(),
                language: UnsetLanguage::All,
            })
        })
        .collect();

    let update_entity = UpdateEntity {
        id: *entity_id.as_bytes(),
        set_properties,
        unset_values: unset_values?,
        context: None,
    };

    let grc20_edit = Grc20Edit {
        id: *Uuid::new_v4().as_bytes(),
        name: edit_name.into(),
        authors: vec![*Uuid::new_v4().as_bytes()],
        created_at: 0,
        ops: vec![Grc20Op::UpdateEntity(update_entity)],
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

/// Generate a CreateEntity operation (using the actual GRC-20 CreateEntity op)
pub fn create_entity_grc20_op(
    edit_name: &str,
    space_id: Uuid,
    entity_id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
    image_url: Option<&str>,
) -> Result<Vec<u8>> {
    let mut values = Vec::new();

    if let Some(name_value) = name {
        values.push(PropertyValue {
            property: *Uuid::parse_str(NAME_PROPERTY_ID)?.as_bytes(),
            value: Grc20Value::Text {
                value: name_value.into(),
                language: None,
            },
        });
    }

    if let Some(desc_value) = description {
        values.push(PropertyValue {
            property: *Uuid::parse_str(DESCRIPTION_PROPERTY_ID)?.as_bytes(),
            value: Grc20Value::Text {
                value: desc_value.into(),
                language: None,
            },
        });
    }

    if let Some(image_url_value) = image_url {
        values.push(PropertyValue {
            property: *Uuid::parse_str(IMAGE_URL_PROPERTY_ID)?.as_bytes(),
            value: Grc20Value::Text {
                value: image_url_value.into(),
                language: None,
            },
        });
    }

    let create_entity = CreateEntity {
        id: *entity_id.as_bytes(),
        values,
        context: None,
    };

    let grc20_edit = Grc20Edit {
        id: *Uuid::new_v4().as_bytes(),
        name: edit_name.into(),
        authors: vec![*Uuid::new_v4().as_bytes()],
        created_at: 0,
        ops: vec![Grc20Op::CreateEntity(create_entity)],
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

/// Generate a single Kafka message containing many UpdateEntity operations.
/// Simulates bulk space imports where one HermesEdit carries thousands of ops.
pub fn create_bulk_entity_edit(
    edit_name: &str,
    space_id: Uuid,
    entities: &[(Uuid, String)],
) -> Result<Vec<u8>> {
    let ops: Vec<Grc20Op> = entities
        .iter()
        .map(|(entity_id, name)| {
            Grc20Op::UpdateEntity(UpdateEntity {
                id: *entity_id.as_bytes(),
                set_properties: vec![PropertyValue {
                    property: *Uuid::parse_str(NAME_PROPERTY_ID)
                        .expect("valid NAME_PROPERTY_ID")
                        .as_bytes(),
                    value: Grc20Value::Text {
                        value: name.clone().into(),
                        language: None,
                    },
                }],
                unset_values: vec![],
                context: None,
            })
        })
        .collect();

    let grc20_edit = Grc20Edit {
        id: *Uuid::new_v4().as_bytes(),
        name: edit_name.into(),
        authors: vec![*Uuid::new_v4().as_bytes()],
        created_at: 0,
        ops,
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

/// Generate a DeleteEntity operation
pub fn delete_entity(edit_name: &str, space_id: Uuid, entity_id: Uuid) -> Result<Vec<u8>> {
    let delete_entity = DeleteEntity {
        id: *entity_id.as_bytes(),
        context: None,
    };

    let grc20_edit = Grc20Edit {
        id: *Uuid::new_v4().as_bytes(),
        name: edit_name.into(),
        authors: vec![*Uuid::new_v4().as_bytes()],
        created_at: 0,
        ops: vec![Grc20Op::DeleteEntity(delete_entity)],
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

/// Generate a RestoreEntity operation (un-delete)
pub fn restore_entity(edit_name: &str, space_id: Uuid, entity_id: Uuid) -> Result<Vec<u8>> {
    let restore_entity = RestoreEntity {
        id: *entity_id.as_bytes(),
        context: None,
    };

    let grc20_edit = Grc20Edit {
        id: *Uuid::new_v4().as_bytes(),
        name: edit_name.into(),
        authors: vec![*Uuid::new_v4().as_bytes()],
        created_at: 0,
        ops: vec![Grc20Op::RestoreEntity(restore_entity)],
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
