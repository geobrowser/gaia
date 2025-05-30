use indexer_utils::id;
use std::collections::HashMap;
use uuid::Uuid;

use grc20::pb::grc20::{op::Payload, options, Edit, Op};

#[derive(Clone)]
pub enum ValueChangeType {
    SET,
    DELETE,
}

#[derive(Clone)]
pub struct ValueOp {
    pub id: String,
    pub change_type: ValueChangeType,
    pub entity_id: String,
    pub property_id: String,
    pub space_id: String,
    pub value: Option<String>,
    pub language: Option<String>,
    pub format: Option<String>,
    pub unit: Option<String>,
    pub timezone: Option<String>,
    pub has_date: Option<bool>,
    pub has_time: Option<bool>,
}

pub struct ValuesModel;

impl ValuesModel {
    pub fn map_edit_to_values(edit: &Edit, space_id: &String) -> (Vec<ValueOp>, Vec<String>) {
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

        return (created, deleted.iter().map(|op| op.id.clone()).collect());
    }
}

fn squash_values(triple_ops: &Vec<ValueOp>) -> Vec<ValueOp> {
    let mut hash = HashMap::new();

    for op in triple_ops {
        hash.insert(op.id.clone(), op.clone());
    }

    let result: Vec<_> = hash.into_values().collect();

    return result;
}

fn derive_value_id(entity_id: &String, property_id: &String, space_id: &String) -> String {
    format!("{}:{}:{}", entity_id, property_id, space_id)
}

fn value_op_from_op(op: &Op, space_id: &String) -> Vec<ValueOp> {
    let mut values = Vec::new();

    if let Some(payload) = &op.payload {
        match payload {
            Payload::UpdateEntity(entity) => {
                let entity_id_bytes = id::transform_id_bytes(entity.id.clone());

                match entity_id_bytes {
                    Ok(entity_id_bytes) => {
                        let entity_id = Uuid::from_bytes(entity_id_bytes).to_string();

                        for value in &entity.values {
                            let property_id_bytes = id::transform_id_bytes(value.property.clone());

                            if let Err(_) = property_id_bytes {
                                tracing::error!(
                                    "[Values][UpdateEntity] Could not transform Vec<u8> for property.id {:?}",
                                    &entity.id
                                );
                                continue;
                            }

                            let property_id =
                                Uuid::from_bytes(property_id_bytes.unwrap()).to_string();

                            let (language, format, unit, timezone, has_date, has_time) =
                                extract_options(&value.options);

                            values.push(ValueOp {
                                id: derive_value_id(&entity_id, &property_id, space_id),
                                change_type: ValueChangeType::SET,
                                property_id,
                                entity_id: entity_id.clone(),
                                space_id: space_id.clone(),
                                value: Some(value.value.clone()),
                                language,
                                format,
                                unit,
                                timezone,
                                has_date,
                                has_time,
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
                        let entity_id = Uuid::from_bytes(entity_id_bytes).to_string();

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
                                Uuid::from_bytes(property_id_bytes.unwrap()).to_string();

                            values.push(ValueOp {
                                id: derive_value_id(&entity_id, &property_id, space_id),
                                change_type: ValueChangeType::DELETE,
                                property_id,
                                entity_id: entity_id.clone(),
                                space_id: space_id.clone(),
                                value: None,
                                language: None,
                                format: None,
                                unit: None,
                                timezone: None,
                                has_date: None,
                                has_time: None,
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
    options: &Option<grc20::pb::ipfs::Options>,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<bool>,
    Option<bool>,
) {
    if let Some(opts) = options {
        if let Some(value) = &opts.value {
            match value {
                options::Value::Text(text_opts) => {
                    let language = text_opts
                        .language
                        .as_ref()
                        .and_then(|lang| String::from_utf8(lang.clone()).ok());
                    (language, None, None, None, None, None)
                }
                options::Value::Number(number_opts) => {
                    let unit = number_opts
                        .unit
                        .as_ref()
                        .and_then(|u| String::from_utf8(u.clone()).ok());
                    (None, number_opts.format.clone(), unit, None, None, None)
                }
                options::Value::Time(time_opts) => {
                    let timezone = time_opts
                        .timezone
                        .as_ref()
                        .and_then(|tz| String::from_utf8(tz.clone()).ok());
                    (
                        None,
                        time_opts.format.clone(),
                        None,
                        timezone,
                        time_opts.has_date,
                        time_opts.has_time,
                    )
                }
            }
        } else {
            (None, None, None, None, None, None)
        }
    } else {
        (None, None, None, None, None, None)
    }
}
