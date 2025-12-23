use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use hermes_schema::pb::knowledge::HermesEdit;
use indexer_utils::id::transform_id_bytes;
use uuid::Uuid;
use wire::pb::grc20::op::Payload;
use wire::pb::grc20::options;

use crate::error::HandlerError;
use crate::models::{
    entities::EntityItem,
    properties::{DataType, PropertyItem},
    relations::{
        DeleteRelationItem, RelationOp, SetRelationItem, UnsetRelationItem, UpdateRelationItem,
    },
    values::{ValueChangeType, ValueOp},
};

/// Result of processing an edit message
pub struct EditResult {
    pub entities: Vec<EntityItem>,
    pub properties: Vec<PropertyItem>,
    pub values: Vec<ValueOp>,
    pub relations: Vec<RelationOp>,
}

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

/// Process a HermesEdit message and return the extracted data
pub fn handle_edit(edit: &HermesEdit) -> Result<EditResult, HandlerError> {
    let space_id = parse_space_id(edit.space_id.as_slice())?;
    let meta = EditMetadata::from_edit(edit);

    // Extract all data from the edit
    let entities = extract_entities(edit, &meta);
    let properties = extract_properties(edit);
    let value_ops = extract_values(edit, &space_id);
    let relation_ops = extract_relations(edit, &space_id);

    // Squash operations within this edit to resolve conflicts
    let values = squash_values(&value_ops);
    let relations = squash_relations(&relation_ops);

    Ok(EditResult {
        entities,
        properties,
        values,
        relations,
    })
}

/// Squash value operations - last operation for each value ID wins
fn squash_values(value_ops: &[ValueOp]) -> Vec<ValueOp> {
    let mut hash: HashMap<Uuid, ValueOp> = HashMap::new();

    for op in value_ops {
        hash.insert(op.id, op.clone());
    }

    hash.into_values().collect()
}

/// Squash relation operations with proper merging logic
fn squash_relations(relation_ops: &[RelationOp]) -> Vec<RelationOp> {
    let mut hash: HashMap<Uuid, RelationOp> = HashMap::new();

    for op in relation_ops {
        let id = op.id();
        if let Some(existing) = hash.get(&id) {
            let merged = merge_relation_ops(existing.clone(), op.clone());
            hash.insert(id, merged);
        } else {
            hash.insert(id, op.clone());
        }
    }

    hash.into_values().collect()
}

/// Merge two relation operations for the same ID
fn merge_relation_ops(existing: RelationOp, new: RelationOp) -> RelationOp {
    match (existing, new.clone()) {
        // create -> create: Use the new create
        (RelationOp::Create(_), RelationOp::Create(_)) => new,

        // create -> update: Merge fields into create
        (RelationOp::Create(c), RelationOp::Update(u)) => RelationOp::Create(SetRelationItem {
            id: c.id,
            entity_id: c.entity_id,
            type_id: c.type_id,
            from_id: c.from_id,
            to_id: c.to_id,
            space_id: c.space_id,
            from_space_id: u.from_space_id.or(c.from_space_id),
            from_version_id: u.from_version_id.or(c.from_version_id),
            to_space_id: u.to_space_id.or(c.to_space_id),
            to_version_id: u.to_version_id.or(c.to_version_id),
            position: u.position.or(c.position),
            verified: u.verified.or(c.verified),
        }),

        // create -> delete: Delete wins
        (RelationOp::Create(_), RelationOp::Delete(d)) => RelationOp::Delete(d),

        // create -> unset: Apply unset to create
        (RelationOp::Create(c), RelationOp::Unset(u)) => RelationOp::Create(SetRelationItem {
            id: c.id,
            entity_id: c.entity_id,
            type_id: c.type_id,
            from_id: c.from_id,
            to_id: c.to_id,
            space_id: c.space_id,
            from_space_id: if u.from_space_id == Some(true) {
                None
            } else {
                c.from_space_id
            },
            from_version_id: if u.from_version_id == Some(true) {
                None
            } else {
                c.from_version_id
            },
            to_space_id: if u.to_space_id == Some(true) {
                None
            } else {
                c.to_space_id
            },
            to_version_id: if u.to_version_id == Some(true) {
                None
            } else {
                c.to_version_id
            },
            position: if u.position == Some(true) {
                None
            } else {
                c.position
            },
            verified: if u.verified == Some(true) {
                None
            } else {
                c.verified
            },
        }),

        // update -> create: Create wins (overwrites)
        (RelationOp::Update(_), RelationOp::Create(_)) => new,

        // update -> update: Merge fields
        (RelationOp::Update(e), RelationOp::Update(u)) => RelationOp::Update(UpdateRelationItem {
            id: e.id,
            space_id: e.space_id,
            from_space_id: u.from_space_id.or(e.from_space_id),
            from_version_id: u.from_version_id.or(e.from_version_id),
            to_space_id: u.to_space_id.or(e.to_space_id),
            to_version_id: u.to_version_id.or(e.to_version_id),
            position: u.position.or(e.position),
            verified: u.verified.or(e.verified),
        }),

        // update -> delete: Delete wins
        (RelationOp::Update(_), RelationOp::Delete(d)) => RelationOp::Delete(d),

        // update -> unset: Apply unset to update
        (RelationOp::Update(e), RelationOp::Unset(u)) => RelationOp::Update(UpdateRelationItem {
            id: e.id,
            space_id: e.space_id,
            from_space_id: if u.from_space_id == Some(true) {
                None
            } else {
                e.from_space_id
            },
            from_version_id: if u.from_version_id == Some(true) {
                None
            } else {
                e.from_version_id
            },
            to_space_id: if u.to_space_id == Some(true) {
                None
            } else {
                e.to_space_id
            },
            to_version_id: if u.to_version_id == Some(true) {
                None
            } else {
                e.to_version_id
            },
            position: if u.position == Some(true) {
                None
            } else {
                e.position
            },
            verified: if u.verified == Some(true) {
                None
            } else {
                e.verified
            },
        }),

        // delete -> anything: New op wins (recreation after delete)
        (RelationOp::Delete(_), _) => new,

        // unset -> create: Create wins
        (RelationOp::Unset(_), RelationOp::Create(_)) => new,

        // unset -> update: Update wins
        (RelationOp::Unset(_), RelationOp::Update(_)) => new,

        // unset -> delete: Delete wins
        (RelationOp::Unset(_), RelationOp::Delete(d)) => RelationOp::Delete(d),

        // unset -> unset: Merge the unset flags
        (RelationOp::Unset(e), RelationOp::Unset(u)) => RelationOp::Unset(UnsetRelationItem {
            id: e.id,
            space_id: e.space_id,
            from_space_id: u.from_space_id.or(e.from_space_id),
            from_version_id: u.from_version_id.or(e.from_version_id),
            to_space_id: u.to_space_id.or(e.to_space_id),
            to_version_id: u.to_version_id.or(e.to_version_id),
            position: u.position.or(e.position),
            verified: u.verified.or(e.verified),
        }),
    }
}

fn parse_space_id(space_id_bytes: &[u8]) -> Result<Uuid, HandlerError> {
    // Convert 16-byte UUID to UUID struct
    if space_id_bytes.len() != 16 {
        return Err(HandlerError::InvalidSpaceId(format!(
            "Expected 16 bytes, got {}",
            space_id_bytes.len()
        )));
    }
    let bytes: [u8; 16] = space_id_bytes.try_into().map_err(|_| {
        HandlerError::InvalidSpaceId("Failed to convert bytes to array".to_string())
    })?;
    Ok(Uuid::from_bytes(bytes))
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

fn extract_values(edit: &HermesEdit, space_id: &Uuid) -> Vec<ValueOp> {
    let mut value_ops = Vec::new();

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

                        value_ops.push(ValueOp {
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
                        });
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

                        value_ops.push(ValueOp {
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
                        });
                    }
                }
                _ => {}
            }
        }
    }

    value_ops
}

fn extract_relations(edit: &HermesEdit, space_id: &Uuid) -> Vec<RelationOp> {
    let mut relation_ops = Vec::new();

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

                    relation_ops.push(RelationOp::Create(SetRelationItem {
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
                    }));
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

                    relation_ops.push(RelationOp::Update(UpdateRelationItem {
                        id: relation_id,
                        space_id: *space_id,
                        position: updated.position.clone(),
                        verified: updated.verified,
                        to_space_id: to_space,
                        from_space_id: from_space,
                        from_version_id: from_version,
                        to_version_id: to_version,
                    }));
                }
                Payload::UnsetRelationFields(unset) => {
                    let relation_id = match transform_id_bytes(unset.id.clone()) {
                        Ok(bytes) => Uuid::from_bytes(bytes),
                        Err(_) => continue,
                    };

                    relation_ops.push(RelationOp::Unset(UnsetRelationItem {
                        id: relation_id,
                        space_id: *space_id,
                        from_space_id: unset.from_space,
                        from_version_id: unset.from_version,
                        to_space_id: unset.to_space,
                        to_version_id: unset.to_version,
                        position: unset.position,
                        verified: unset.verified,
                    }));
                }
                Payload::DeleteRelation(relation_id_bytes) => {
                    if let Ok(bytes) = transform_id_bytes(relation_id_bytes.clone()) {
                        relation_ops.push(RelationOp::Delete(DeleteRelationItem {
                            id: Uuid::from_bytes(bytes),
                            space_id: *space_id,
                        }));
                    }
                }
                _ => {}
            }
        }
    }

    relation_ops
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_value_op(id: Uuid, change_type: ValueChangeType, value: Option<String>) -> ValueOp {
        ValueOp {
            id,
            change_type,
            entity_id: Uuid::new_v4(),
            property_id: Uuid::new_v4(),
            space_id: Uuid::new_v4(),
            language: None,
            unit: None,
            string: value,
            number: None,
            boolean: None,
            time: None,
            point: None,
        }
    }

    fn make_create_relation(id: Uuid) -> SetRelationItem {
        SetRelationItem {
            id,
            entity_id: Uuid::new_v4(),
            type_id: Uuid::new_v4(),
            from_id: Uuid::new_v4(),
            to_id: Uuid::new_v4(),
            space_id: Uuid::new_v4(),
            from_space_id: None,
            from_version_id: None,
            to_space_id: None,
            to_version_id: None,
            position: None,
            verified: None,
        }
    }

    fn make_update_relation(id: Uuid, space_id: Uuid) -> UpdateRelationItem {
        UpdateRelationItem {
            id,
            space_id,
            from_space_id: None,
            from_version_id: None,
            to_space_id: None,
            to_version_id: None,
            position: None,
            verified: None,
        }
    }

    fn make_unset_relation(id: Uuid, space_id: Uuid) -> UnsetRelationItem {
        UnsetRelationItem {
            id,
            space_id,
            from_space_id: None,
            from_version_id: None,
            to_space_id: None,
            to_version_id: None,
            position: None,
            verified: None,
        }
    }

    fn make_delete_relation(id: Uuid, space_id: Uuid) -> DeleteRelationItem {
        DeleteRelationItem { id, space_id }
    }

    // ===================
    // Value squashing tests
    // ===================

    #[test]
    fn test_squash_values_empty() {
        let ops: Vec<ValueOp> = vec![];
        let result = squash_values(&ops);
        assert!(result.is_empty());
    }

    #[test]
    fn test_squash_values_single_set() {
        let id = Uuid::new_v4();
        let ops = vec![make_value_op(
            id,
            ValueChangeType::Set,
            Some("hello".into()),
        )];
        let result = squash_values(&ops);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0].change_type, ValueChangeType::Set));
    }

    #[test]
    fn test_squash_values_set_then_delete_same_id() {
        let id = Uuid::new_v4();
        let ops = vec![
            make_value_op(id, ValueChangeType::Set, Some("hello".into())),
            make_value_op(id, ValueChangeType::Delete, None),
        ];
        let result = squash_values(&ops);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0].change_type, ValueChangeType::Delete));
    }

    #[test]
    fn test_squash_values_delete_then_set_same_id() {
        let id = Uuid::new_v4();
        let ops = vec![
            make_value_op(id, ValueChangeType::Delete, None),
            make_value_op(id, ValueChangeType::Set, Some("recreated".into())),
        ];
        let result = squash_values(&ops);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0].change_type, ValueChangeType::Set));
        assert_eq!(result[0].string, Some("recreated".into()));
    }

    #[test]
    fn test_squash_values_multiple_sets_same_id() {
        let id = Uuid::new_v4();
        let ops = vec![
            make_value_op(id, ValueChangeType::Set, Some("first".into())),
            make_value_op(id, ValueChangeType::Set, Some("second".into())),
            make_value_op(id, ValueChangeType::Set, Some("third".into())),
        ];
        let result = squash_values(&ops);
        assert_eq!(result.len(), 1);
        // Last one wins
        assert_eq!(result[0].string, Some("third".into()));
    }

    #[test]
    fn test_squash_values_different_ids_preserved() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let ops = vec![
            make_value_op(id1, ValueChangeType::Set, Some("value1".into())),
            make_value_op(id2, ValueChangeType::Set, Some("value2".into())),
        ];
        let result = squash_values(&ops);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_squash_values_mixed_operations() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let id3 = Uuid::new_v4();
        let ops = vec![
            make_value_op(id1, ValueChangeType::Set, Some("v1".into())),
            make_value_op(id2, ValueChangeType::Set, Some("v2".into())),
            make_value_op(id1, ValueChangeType::Delete, None), // id1 gets deleted
            make_value_op(id3, ValueChangeType::Set, Some("v3".into())),
            make_value_op(id2, ValueChangeType::Set, Some("v2-updated".into())), // id2 updated
        ];
        let result = squash_values(&ops);
        assert_eq!(result.len(), 3);

        let id1_op = result.iter().find(|op| op.id == id1).unwrap();
        assert!(matches!(id1_op.change_type, ValueChangeType::Delete));

        let id2_op = result.iter().find(|op| op.id == id2).unwrap();
        assert_eq!(id2_op.string, Some("v2-updated".into()));

        let id3_op = result.iter().find(|op| op.id == id3).unwrap();
        assert_eq!(id3_op.string, Some("v3".into()));
    }

    // ===================
    // Relation squashing tests
    // ===================

    #[test]
    fn test_squash_relations_empty() {
        let ops: Vec<RelationOp> = vec![];
        let result = squash_relations(&ops);
        assert!(result.is_empty());
    }

    #[test]
    fn test_squash_relations_single_create() {
        let id = Uuid::new_v4();
        let ops = vec![RelationOp::Create(make_create_relation(id))];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], RelationOp::Create(_)));
    }

    #[test]
    fn test_squash_relations_create_then_delete() {
        let id = Uuid::new_v4();
        let space_id = Uuid::new_v4();
        let mut create = make_create_relation(id);
        create.space_id = space_id;
        let ops = vec![
            RelationOp::Create(create),
            RelationOp::Delete(make_delete_relation(id, space_id)),
        ];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], RelationOp::Delete(_)));
    }

    #[test]
    fn test_squash_relations_delete_then_create() {
        let id = Uuid::new_v4();
        let space_id = Uuid::new_v4();
        let ops = vec![
            RelationOp::Delete(make_delete_relation(id, space_id)),
            RelationOp::Create(make_create_relation(id)),
        ];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], RelationOp::Create(_)));
    }

    #[test]
    fn test_squash_relations_create_then_update_merges_fields() {
        let id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let mut create = make_create_relation(id);
        create.space_id = space_id;
        create.position = Some("original".into());

        let mut update = make_update_relation(id, space_id);
        update.position = Some("updated".into());
        update.verified = Some(true);

        let ops = vec![RelationOp::Create(create), RelationOp::Update(update)];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 1);

        if let RelationOp::Create(r) = &result[0] {
            assert_eq!(r.position, Some("updated".into()));
            assert_eq!(r.verified, Some(true));
        } else {
            panic!("Expected Create variant");
        }
    }

    #[test]
    fn test_squash_relations_update_then_update_merges_fields() {
        let id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let mut update1 = make_update_relation(id, space_id);
        update1.position = Some("pos1".into());
        update1.from_space_id = Some("from_space".into());

        let mut update2 = make_update_relation(id, space_id);
        update2.position = Some("pos2".into());
        update2.to_space_id = Some("to_space".into());

        let ops = vec![RelationOp::Update(update1), RelationOp::Update(update2)];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 1);

        if let RelationOp::Update(r) = &result[0] {
            assert_eq!(r.position, Some("pos2".into())); // Second update wins
            assert_eq!(r.from_space_id, Some("from_space".into())); // Preserved from first
            assert_eq!(r.to_space_id, Some("to_space".into())); // From second
        } else {
            panic!("Expected Update variant");
        }
    }

    #[test]
    fn test_squash_relations_update_then_delete() {
        let id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let update = make_update_relation(id, space_id);
        let ops = vec![
            RelationOp::Update(update),
            RelationOp::Delete(make_delete_relation(id, space_id)),
        ];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], RelationOp::Delete(_)));
    }

    #[test]
    fn test_squash_relations_create_then_unset_clears_fields() {
        let id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let mut create = make_create_relation(id);
        create.space_id = space_id;
        create.position = Some("has_position".into());
        create.verified = Some(true);

        let mut unset = make_unset_relation(id, space_id);
        unset.position = Some(true); // Unset position
        unset.verified = Some(false); // Don't unset verified

        let ops = vec![RelationOp::Create(create), RelationOp::Unset(unset)];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 1);

        if let RelationOp::Create(r) = &result[0] {
            assert_eq!(r.position, None); // Was unset
            assert_eq!(r.verified, Some(true)); // Was preserved
        } else {
            panic!("Expected Create variant");
        }
    }

    #[test]
    fn test_squash_relations_unset_then_unset_merges() {
        let id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let mut unset1 = make_unset_relation(id, space_id);
        unset1.position = Some(true);

        let mut unset2 = make_unset_relation(id, space_id);
        unset2.verified = Some(true);

        let ops = vec![RelationOp::Unset(unset1), RelationOp::Unset(unset2)];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 1);

        if let RelationOp::Unset(r) = &result[0] {
            assert_eq!(r.position, Some(true));
            assert_eq!(r.verified, Some(true));
        } else {
            panic!("Expected Unset variant");
        }
    }

    #[test]
    fn test_squash_relations_unset_then_create_overwrites() {
        let id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let unset = make_unset_relation(id, space_id);
        let create = make_create_relation(id);

        let ops = vec![RelationOp::Unset(unset), RelationOp::Create(create)];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], RelationOp::Create(_)));
    }

    #[test]
    fn test_squash_relations_different_ids_preserved() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();

        let ops = vec![
            RelationOp::Create(make_create_relation(id1)),
            RelationOp::Create(make_create_relation(id2)),
        ];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_squash_relations_complex_sequence() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let mut create1 = make_create_relation(id1);
        create1.space_id = space_id;

        let mut create2 = make_create_relation(id2);
        create2.space_id = space_id;

        let mut update1 = make_update_relation(id1, space_id);
        update1.position = Some("pos".into());

        let ops = vec![
            RelationOp::Create(create1),
            RelationOp::Create(create2),
            RelationOp::Update(update1),
            RelationOp::Delete(make_delete_relation(id2, space_id)), // id2 created then deleted
        ];
        let result = squash_relations(&ops);
        assert_eq!(result.len(), 2);

        // id1 should be Create with merged position
        let id1_op = result.iter().find(|op| op.id() == id1).unwrap();
        if let RelationOp::Create(r) = id1_op {
            assert_eq!(r.position, Some("pos".into()));
        } else {
            panic!("Expected Create for id1");
        }

        // id2 should be Delete
        let id2_op = result.iter().find(|op| op.id() == id2).unwrap();
        assert!(matches!(id2_op, RelationOp::Delete(_)));
    }
}
