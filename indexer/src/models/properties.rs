use std::collections::HashMap;

use grc20::pb::ipfsv2::{Edit, Op, OpType, Options};

#[derive(Clone)]
pub enum PropertyChangeType {
    SET,
    DELETE,
}

#[derive(Clone)]
pub struct PropertyOp {
    pub id: String,
    pub change_type: PropertyChangeType,
    pub entity_id: String,
    pub attribute_id: String,
    pub space_id: String,
    pub text_value: Option<String>,
    pub format_option: Option<String>,
    pub unit_option: Option<String>,
}

pub struct PropertiesModel;

impl PropertiesModel {
    pub fn map_edit_to_properties(
        edit: &Edit,
        space_id: &String,
    ) -> (Vec<PropertyOp>, Vec<String>) {
        let mut triple_ops: Vec<PropertyOp> = Vec::new();

        for op in &edit.ops {
            if let Some(triple_op) = property_op_from_op(op, space_id) {
                triple_ops.push(triple_op);
            }
        }

        let squashed = squash_properties(&triple_ops);
        let (created, deleted): (Vec<PropertyOp>, Vec<PropertyOp>) = squashed
            .into_iter()
            .partition(|op| matches!(op.change_type, PropertyChangeType::SET));

        return (created, deleted.iter().map(|op| op.id.clone()).collect());
    }
}

fn squash_properties(triple_ops: &Vec<PropertyOp>) -> Vec<PropertyOp> {
    let mut hash = HashMap::new();

    for op in triple_ops {
        hash.insert(op.id.clone(), op.clone());
    }

    let result: Vec<_> = hash.into_values().collect();

    return result;
}

fn derive_property_id(entity_id: &String, attribute_id: &String, space_id: &String) -> String {
    format!("{}:{}:{}", entity_id, attribute_id, space_id)
}

fn property_op_from_op(op: &Op, space_id: &String) -> Option<PropertyOp> {
    if let Ok(op_type) = OpType::try_from(op.r#type) {
        return match op_type {
            // SET_TRIPLE
            OpType::UpdateEntity | OpType::CreateEntity => {
                if let Some(entity) = op.entity.clone() {
                    let entity_id = String::from_utf8(entity.id);

                    if let Ok(entity_id) = entity_id {
                        for value in &entity.values {
                            let property_id = String::from_utf8(value.property_id.clone());

                            if let Ok(property_id) = property_id {
                                let triple_values = map_property_value(&value.value);
                                let triple_value_options =
                                    &value.options.clone().unwrap_or(Options {
                                        format: vec![],
                                        unit: vec![],
                                    });

                                if let Some(triple_values) = triple_values {
                                    return Some(PropertyOp {
                                        id: derive_property_id(&entity_id, &property_id, space_id),
                                        change_type: PropertyChangeType::SET,
                                        attribute_id: property_id,
                                        entity_id,
                                        space_id: space_id.clone(),
                                        text_value: Some(triple_values.text_value),
                                        unit_option: String::from_utf8(
                                            triple_value_options.unit.clone(),
                                        )
                                        .ok(),
                                        format_option: String::from_utf8(
                                            triple_value_options.format.clone(),
                                        )
                                        .ok(),
                                    });
                                }
                            }
                        }
                    }
                }

                return None;
            }
            OpType::UnsetProperties => {
                if let Some(entity) = op.entity.clone() {
                    let entity_id = String::from_utf8(entity.id);

                    if let Ok(entity_id) = entity_id {
                        for value in &entity.values {
                            let property_id = String::from_utf8(value.property_id.clone());

                            if let Ok(property_id) = property_id {
                                return Some(PropertyOp {
                                    id: derive_property_id(&entity_id, &property_id, space_id),
                                    change_type: PropertyChangeType::DELETE,
                                    attribute_id: property_id,
                                    entity_id,
                                    space_id: space_id.clone(),
                                    text_value: None,
                                    unit_option: None,
                                    format_option: None,
                                });
                            }
                        }
                    }
                }

                return None;
            }
            _ => None,
        };
    };

    None
}

struct TripleValues {
    text_value: String,
}

fn map_property_value(value: &Option<Vec<u8>>) -> Option<TripleValues> {
    // value type isn't a thing now so we only have text values
    // for everything now?

    match value {
        Some(value) => {
            let value = String::from_utf8(value.clone());

            match value {
                Ok(value) => Some(TripleValues { text_value: value }),
                Err(_) => None,
            }
        }
        None => None,
    }
}
