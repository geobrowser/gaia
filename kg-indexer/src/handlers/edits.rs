use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;

use hermes_schema::pb::knowledge::HermesEdit;
use indexer_utils::id::transform_id_bytes;
use tracing::debug;
use uuid::Uuid;
use wire::pb::grc20::op::Payload;
use wire::pb::grc20::options;

use crate::error::IndexerError;
use crate::models::{
    entities::EntityItem,
    properties::{DataType, PropertyItem},
    relations::{SetRelationItem, UnsetRelationItem, UpdateRelationItem},
    values::{ValueChangeType, ValueOp},
};
use crate::storage::Storage;

/// Metadata extracted from the HermesEdit for timestamping
pub struct EditMetadata {
    pub timestamp: String,
    pub block_number: String,
}

impl EditMetadata {
    pub fn from_edit(edit: &HermesEdit) -> Self {
        let (timestamp, block_number) = edit
            .meta
            .as_ref()
            .map(|m| (m.created_at.to_string(), m.block_number.to_string()))
            .unwrap_or_else(|| ("0".to_string(), "0".to_string()));

        Self {
            timestamp,
            block_number,
        }
    }
}

/// Process a HermesEdit message and write to storage
pub async fn handle_edit(
    edit: &HermesEdit,
    storage: &Storage,
) -> Result<(), IndexerError> {
    let space_id = parse_space_id(&edit.space_id)?;
    let meta = EditMetadata::from_edit(edit);

    // Extract all data from the edit
    let entities = extract_entities(edit, &meta);
    let properties = extract_properties(edit);
    let (set_values, delete_value_ids) = extract_values(edit, &space_id);
    let (set_relations, update_relations, unset_relations, delete_relation_ids) =
        extract_relations(edit, &space_id);

    // Write to database in a transaction
    let mut tx = storage.pool.begin().await?;

    // Insert entities first (they may be referenced by values/relations)
    if !entities.is_empty() {
        storage.insert_entities(&entities, &mut tx).await?;
    }

    // Insert properties (must exist before values reference them)
    if !properties.is_empty() {
        storage.insert_properties(&properties, &mut tx).await?;
    }

    // Insert/delete values
    if !set_values.is_empty() {
        storage.insert_values(&set_values, &mut tx).await?;
    }
    if !delete_value_ids.is_empty() {
        storage.delete_values(&delete_value_ids, &space_id, &mut tx).await?;
    }

    // Insert/update/delete relations
    if !set_relations.is_empty() {
        storage.insert_relations(&set_relations, &mut tx).await?;
    }
    if !update_relations.is_empty() {
        storage.update_relations(&update_relations, &mut tx).await?;
    }
    if !unset_relations.is_empty() {
        storage.unset_relation_fields(&unset_relations, &mut tx).await?;
    }
    if !delete_relation_ids.is_empty() {
        storage.delete_relations(&delete_relation_ids, &space_id, &mut tx).await?;
    }

    tx.commit().await?;

    debug!(
        space_id = %space_id,
        entity_count = entities.len(),
        property_count = properties.len(),
        value_set_count = set_values.len(),
        value_delete_count = delete_value_ids.len(),
        relation_set_count = set_relations.len(),
        "Processed edit"
    );

    Ok(())
}

fn parse_space_id(space_id_str: &str) -> Result<Uuid, IndexerError> {
    // Handle hex-encoded UUID bytes (32 hex chars) or standard UUID format
    if space_id_str.len() == 32 && space_id_str.chars().all(|c| c.is_ascii_hexdigit()) {
        let uuid_str = format!(
            "{}-{}-{}-{}-{}",
            &space_id_str[0..8],
            &space_id_str[8..12],
            &space_id_str[12..16],
            &space_id_str[16..20],
            &space_id_str[20..32]
        );
        Uuid::parse_str(&uuid_str)
            .map_err(|e| IndexerError::parse(format!("Invalid hex-encoded space_id: {}", e)))
    } else {
        Uuid::parse_str(space_id_str)
            .map_err(|e| IndexerError::parse(format!("Invalid space_id: {}", e)))
    }
}

fn extract_entities(edit: &HermesEdit, meta: &EditMetadata) -> Vec<EntityItem> {
    let mut entities = Vec::new();
    let mut seen = HashSet::new();

    for op in &edit.ops {
        if let Some(payload) = &op.payload {
            let ids_to_add: Vec<Uuid> = match payload {
                Payload::UpdateEntity(entity) => {
                    let mut ids = Vec::new();
                    if let Ok(bytes) = transform_id_bytes(entity.id.clone()) {
                        ids.push(Uuid::from_bytes(bytes));
                    }
                    for value in &entity.values {
                        if let Ok(bytes) = transform_id_bytes(value.property.clone()) {
                            ids.push(Uuid::from_bytes(bytes));
                        }
                    }
                    ids
                }
                Payload::UnsetEntityValues(entity) => {
                    let mut ids = Vec::new();
                    if let Ok(bytes) = transform_id_bytes(entity.id.clone()) {
                        ids.push(Uuid::from_bytes(bytes));
                    }
                    for prop in &entity.properties {
                        if let Ok(bytes) = transform_id_bytes(prop.clone()) {
                            ids.push(Uuid::from_bytes(bytes));
                        }
                    }
                    ids
                }
                Payload::CreateRelation(relation) => {
                    let mut ids = Vec::new();
                    for bytes_vec in [
                        &relation.id,
                        &relation.entity,
                        &relation.r#type,
                        &relation.from_entity,
                        &relation.to_entity,
                    ] {
                        if let Ok(bytes) = transform_id_bytes(bytes_vec.clone()) {
                            ids.push(Uuid::from_bytes(bytes));
                        }
                    }
                    ids
                }
                Payload::DeleteRelation(relation_id) => {
                    if let Ok(bytes) = transform_id_bytes(relation_id.clone()) {
                        vec![Uuid::from_bytes(bytes)]
                    } else {
                        vec![]
                    }
                }
                _ => vec![],
            };

            for id in ids_to_add {
                if !seen.contains(&id) {
                    entities.push(EntityItem {
                        id,
                        created_at: meta.timestamp.clone(),
                        created_at_block: meta.block_number.clone(),
                        updated_at: meta.timestamp.clone(),
                        updated_at_block: meta.block_number.clone(),
                    });
                    seen.insert(id);
                }
            }
        }
    }

    entities
}

fn extract_properties(edit: &HermesEdit) -> Vec<PropertyItem> {
    let mut properties = Vec::new();
    let mut seen = HashMap::new();

    for op in &edit.ops {
        if let Some(Payload::CreateProperty(property)) = &op.payload {
            if let Ok(bytes) = transform_id_bytes(property.id.clone()) {
                let id = Uuid::from_bytes(bytes);
                if let Ok(data_type) = DataType::try_from(property.data_type) {
                    seen.insert(id, PropertyItem { id, data_type });
                }
            }
        }
    }

    properties.extend(seen.into_values());
    properties
}

fn extract_values(edit: &HermesEdit, space_id: &Uuid) -> (Vec<ValueOp>, Vec<Uuid>) {
    let mut set_values = Vec::new();
    let mut delete_ids = Vec::new();
    let mut seen: HashMap<Uuid, ValueOp> = HashMap::new();

    for op in &edit.ops {
        if let Some(payload) = &op.payload {
            match payload {
                Payload::UpdateEntity(entity) => {
                    let entity_id = match transform_id_bytes(entity.id.clone()) {
                        Ok(bytes) => Uuid::from_bytes(bytes),
                        Err(_) => continue,
                    };

                    for value in &entity.values {
                        let property_id = match transform_id_bytes(value.property.clone()) {
                            Ok(bytes) => Uuid::from_bytes(bytes),
                            Err(_) => continue,
                        };

                        let (language, unit) = extract_options(&value.options);
                        let value_id = derive_value_id(&entity_id, &property_id, space_id);

                        let value_op = ValueOp {
                            id: value_id,
                            change_type: ValueChangeType::Set,
                            entity_id,
                            property_id,
                            space_id: *space_id,
                            language,
                            unit,
                            string: Some(value.value.clone()),
                            number: None,
                            boolean: None,
                            time: None,
                            point: None,
                        };

                        seen.insert(value_id, value_op);
                    }
                }
                Payload::UnsetEntityValues(entity) => {
                    let entity_id = match transform_id_bytes(entity.id.clone()) {
                        Ok(bytes) => Uuid::from_bytes(bytes),
                        Err(_) => continue,
                    };

                    for prop in &entity.properties {
                        let property_id = match transform_id_bytes(prop.clone()) {
                            Ok(bytes) => Uuid::from_bytes(bytes),
                            Err(_) => continue,
                        };

                        let value_id = derive_value_id(&entity_id, &property_id, space_id);

                        let value_op = ValueOp {
                            id: value_id,
                            change_type: ValueChangeType::Delete,
                            entity_id,
                            property_id,
                            space_id: *space_id,
                            language: None,
                            unit: None,
                            string: None,
                            number: None,
                            boolean: None,
                            time: None,
                            point: None,
                        };

                        seen.insert(value_id, value_op);
                    }
                }
                _ => {}
            }
        }
    }

    for (id, op) in seen {
        match op.change_type {
            ValueChangeType::Set => set_values.push(op),
            ValueChangeType::Delete => delete_ids.push(id),
        }
    }

    (set_values, delete_ids)
}

fn extract_relations(
    edit: &HermesEdit,
    space_id: &Uuid,
) -> (Vec<SetRelationItem>, Vec<UpdateRelationItem>, Vec<UnsetRelationItem>, Vec<Uuid>) {
    let mut set_relations = Vec::new();
    let mut update_relations = Vec::new();
    let mut unset_relations = Vec::new();
    let mut delete_ids = Vec::new();

    for op in &edit.ops {
        if let Some(payload) = &op.payload {
            match payload {
                Payload::CreateRelation(relation) => {
                    let relation_id = match transform_id_bytes(relation.id.clone()) {
                        Ok(bytes) => Uuid::from_bytes(bytes),
                        Err(_) => continue,
                    };
                    let entity_id = match transform_id_bytes(relation.entity.clone()) {
                        Ok(bytes) => Uuid::from_bytes(bytes),
                        Err(_) => continue,
                    };
                    let type_id = match transform_id_bytes(relation.r#type.clone()) {
                        Ok(bytes) => Uuid::from_bytes(bytes),
                        Err(_) => continue,
                    };
                    let from_id = match transform_id_bytes(relation.from_entity.clone()) {
                        Ok(bytes) => Uuid::from_bytes(bytes),
                        Err(_) => continue,
                    };
                    let to_id = match transform_id_bytes(relation.to_entity.clone()) {
                        Ok(bytes) => Uuid::from_bytes(bytes),
                        Err(_) => continue,
                    };

                    let to_space = relation
                        .to_space
                        .clone()
                        .and_then(|s| transform_id_bytes(s).ok())
                        .map(|s| Uuid::from_bytes(s).to_string());

                    let from_space = relation
                        .from_space
                        .clone()
                        .and_then(|s| transform_id_bytes(s).ok())
                        .map(|s| Uuid::from_bytes(s).to_string());

                    let from_version = relation
                        .from_version
                        .clone()
                        .and_then(|s| transform_id_bytes(s).ok())
                        .map(|s| Uuid::from_bytes(s).to_string());

                    let to_version = relation
                        .to_version
                        .clone()
                        .and_then(|s| transform_id_bytes(s).ok())
                        .map(|s| Uuid::from_bytes(s).to_string());

                    set_relations.push(SetRelationItem {
                        id: relation_id,
                        entity_id,
                        space_id: *space_id,
                        position: relation.position.clone(),
                        type_id,
                        from_id,
                        from_space_id: from_space,
                        from_version_id: from_version,
                        to_id,
                        to_space_id: to_space,
                        to_version_id: to_version,
                        verified: relation.verified,
                    });
                }
                Payload::UpdateRelation(updated) => {
                    let relation_id = match transform_id_bytes(updated.id.clone()) {
                        Ok(bytes) => Uuid::from_bytes(bytes),
                        Err(_) => continue,
                    };

                    let to_space = updated
                        .to_space
                        .clone()
                        .and_then(|s| transform_id_bytes(s).ok())
                        .map(|s| Uuid::from_bytes(s).to_string());

                    let from_space = updated
                        .from_space
                        .clone()
                        .and_then(|s| transform_id_bytes(s).ok())
                        .map(|s| Uuid::from_bytes(s).to_string());

                    let from_version = updated
                        .from_version
                        .clone()
                        .and_then(|s| transform_id_bytes(s).ok())
                        .map(|s| Uuid::from_bytes(s).to_string());

                    let to_version = updated
                        .to_version
                        .clone()
                        .and_then(|s| transform_id_bytes(s).ok())
                        .map(|s| Uuid::from_bytes(s).to_string());

                    update_relations.push(UpdateRelationItem {
                        id: relation_id,
                        space_id: *space_id,
                        position: updated.position.clone(),
                        verified: updated.verified,
                        to_space_id: to_space,
                        from_space_id: from_space,
                        from_version_id: from_version,
                        to_version_id: to_version,
                    });
                }
                Payload::UnsetRelationFields(unset) => {
                    let relation_id = match transform_id_bytes(unset.id.clone()) {
                        Ok(bytes) => Uuid::from_bytes(bytes),
                        Err(_) => continue,
                    };

                    unset_relations.push(UnsetRelationItem {
                        id: relation_id,
                        space_id: *space_id,
                        from_space_id: unset.from_space,
                        from_version_id: unset.from_version,
                        to_space_id: unset.to_space,
                        to_version_id: unset.to_version,
                        position: unset.position,
                        verified: unset.verified,
                    });
                }
                Payload::DeleteRelation(relation_id_bytes) => {
                    if let Ok(bytes) = transform_id_bytes(relation_id_bytes.clone()) {
                        delete_ids.push(Uuid::from_bytes(bytes));
                    }
                }
                _ => {}
            }
        }
    }

    (set_relations, update_relations, unset_relations, delete_ids)
}

fn derive_value_id(entity_id: &Uuid, property_id: &Uuid, space_id: &Uuid) -> Uuid {
    let mut hasher = DefaultHasher::new();
    entity_id.hash(&mut hasher);
    property_id.hash(&mut hasher);
    space_id.hash(&mut hasher);
    let hash_value = hasher.finish();

    let mut bytes = [0u8; 16];
    bytes[0..8].copy_from_slice(&hash_value.to_be_bytes());
    bytes[8..16].copy_from_slice(&hash_value.to_be_bytes());

    Uuid::from_bytes(bytes)
}

fn extract_options(options: &Option<wire::pb::grc20::Options>) -> (Option<String>, Option<String>) {
    if let Some(opts) = options {
        if let Some(value) = &opts.value {
            match value {
                options::Value::Text(text_opts) => {
                    let language = text_opts
                        .language
                        .as_ref()
                        .and_then(|lang| String::from_utf8(lang.clone()).ok());
                    (language, None)
                }
                options::Value::Number(number_opts) => {
                    let unit = number_opts.unit.as_ref().and_then(|unit_bytes| {
                        match transform_id_bytes(unit_bytes.clone()) {
                            Ok(uuid_bytes) => {
                                let uuid = Uuid::from_bytes(uuid_bytes);
                                Some(uuid.to_string())
                            }
                            Err(_) => None,
                        }
                    });
                    (None, unit)
                }
            }
        } else {
            (None, None)
        }
    } else {
        (None, None)
    }
}
