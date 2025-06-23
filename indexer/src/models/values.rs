use grc20::pb::grc20::{op::Payload, options, Edit, Op};
use indexer_utils::id;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use uuid::Uuid;

#[derive(Clone)]
pub enum ValueChangeType {
    SET,
    DELETE,
}

#[derive(Clone)]
pub struct ValueOp {
    pub id: Uuid,
    pub change_type: ValueChangeType,
    pub entity_id: Uuid,
    pub property_id: Uuid,
    pub space_id: Uuid,
    pub value: Option<String>,
    pub language: Option<String>,
    pub unit: Option<String>,
}

pub struct ValuesModel;

impl ValuesModel {
    pub fn map_edit_to_values(edit: &Edit, space_id: &Uuid) -> (Vec<ValueOp>, Vec<Uuid>) {
        let mut triple_ops: Vec<ValueOp> = Vec::new();

        for op in &edit.ops {
            for op in value_op_from_op(op, space_id) {
                triple_ops.push(op);
            }
        }

        // A single edit may have multiple CREATE, UPDATE, and UNSET value ops applied
        // to the same entity + property id. We need to squash them down into a single
        // op so we can write to the db atomically using the final state of the ops.
        //
        // Ordering of these to-be-squashed ops matters. We use what the order is in
        // the edit.
        let squashed = squash_values(&triple_ops);

        let (created, deleted): (Vec<ValueOp>, Vec<ValueOp>) = squashed
            .into_iter()
            .partition(|op| matches!(op.change_type, ValueChangeType::SET));

        return (created, deleted.iter().map(|op| op.id).collect());
    }
}

fn squash_values(triple_ops: &Vec<ValueOp>) -> Vec<ValueOp> {
    let mut hash = HashMap::new();

    for op in triple_ops {
        hash.insert(op.id, op.clone());
    }

    let result: Vec<_> = hash.into_values().collect();

    return result;
}

fn derive_value_id(entity_id: &Uuid, property_id: &Uuid, space_id: &Uuid) -> Uuid {
    let mut hasher = DefaultHasher::new();
    entity_id.hash(&mut hasher);
    property_id.hash(&mut hasher);
    space_id.hash(&mut hasher);
    let hash_value = hasher.finish();

    // Create a deterministic UUID from the hash
    let mut bytes = [0u8; 16];
    bytes[0..8].copy_from_slice(&hash_value.to_be_bytes());
    bytes[8..16].copy_from_slice(&hash_value.to_be_bytes());

    Uuid::from_bytes(bytes)
}

fn value_op_from_op(op: &Op, space_id: &Uuid) -> Vec<ValueOp> {
    let mut values = Vec::new();

    if let Some(payload) = &op.payload {
        match payload {
            Payload::UpdateEntity(entity) => {
                let entity_id_bytes = id::transform_id_bytes(entity.id.clone());

                match entity_id_bytes {
                    Ok(entity_id_bytes) => {
                        let entity_id = Uuid::from_bytes(entity_id_bytes);

                        for value in &entity.values {
                            let property_id_bytes = id::transform_id_bytes(value.property.clone());

                            if let Err(_) = property_id_bytes {
                                tracing::error!(
                                    "[Values][UpdateEntity] Could not transform Vec<u8> for property.id {:?}",
                                    &entity.id
                                );
                                continue;
                            }

                            let property_id = Uuid::from_bytes(property_id_bytes.unwrap());

                            let (language, unit) = extract_options(&value.options);

                            values.push(ValueOp {
                                id: derive_value_id(&entity_id, &property_id, space_id),
                                change_type: ValueChangeType::SET,
                                property_id,
                                entity_id,
                                space_id: space_id.clone(),
                                value: Some(value.value.clone()),
                                language,
                                unit,
                            });
                        }
                    }
                    Err(_) => tracing::error!(
                        "[Values][UpdateEntity] Could not transform Vec<u8> for entity.id {:?}",
                        &entity.id
                    ),
                }
            }
            Payload::UnsetEntityValues(entity) => {
                let entity_id_bytes = id::transform_id_bytes(entity.id.clone());

                match entity_id_bytes {
                    Ok(entity_id_bytes) => {
                        let entity_id = Uuid::from_bytes(entity_id_bytes);

                        for property in &entity.properties {
                            let property_id_bytes =
                                id::transform_id_bytes(property.clone());

                            if let Err(_) = property_id_bytes {
                                tracing::error!(
                                    "[Values][UnsetEntityValues] Could not transform Vec<u8> for property id {:?}",
                                    &property
                                );
                                continue;
                            }

                            let property_id =
                                Uuid::from_bytes(property_id_bytes.unwrap());

                            values.push(ValueOp {
                                id: derive_value_id(&entity_id, &property_id, space_id),
                                change_type: ValueChangeType::DELETE,
                                property_id,
                                entity_id,
                                space_id: space_id.clone(),
                                value: None,
                                language: None,
                                unit: None,
                            });
                        }
                    },
                    Err(_) => tracing::error!(
                        "[Values][UnsetEntityValues] Could not transform Vec<u8> for entity.id {:?}",
                        &entity.id
                    )
                }
            }
            _ => {}
        };
    }

    return values;
}

fn extract_options(
    options: &Option<grc20::pb::grc20::Options>,
) -> (Option<String>, Option<String>) {
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
                        // If UTF-8 conversion fails, try treating it as raw UUID bytes
                        match id::transform_id_bytes(unit_bytes.clone()) {
                            Ok(uuid_bytes) => {
                                let uuid = Uuid::from_bytes(uuid_bytes);
                                return Some(uuid.to_string());
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
