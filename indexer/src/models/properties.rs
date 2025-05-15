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
    pub property_id: String,
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
            OpType::UpdateEntity | OpType::CreateEntity => {
                if let Some(entity) = op.entity.clone() {
                    if let Ok(entity_id) = String::from_utf8(entity.id) {
                        for value in &entity.values {
                            let property_id = String::from_utf8(value.property_id.clone());

                            if let Ok(property_id) = property_id {
                                let triple_value_options =
                                    &value.options.clone().unwrap_or(Options {
                                        format: None,
                                        unit: None,
                                    });

                                return Some(PropertyOp {
                                    id: derive_property_id(&entity_id, &property_id, space_id),
                                    change_type: PropertyChangeType::SET,
                                    property_id,
                                    entity_id,
                                    space_id: space_id.clone(),
                                    text_value: value.value.clone(),
                                    unit_option: triple_value_options.unit.clone(),
                                    format_option: triple_value_options.format.clone(),
                                });
                            }
                        }
                    }
                }

                return None;
            }
            OpType::UnsetProperties => {
                if let Some(entity) = op.entity.clone() {
                    if let Ok(entity_id) = String::from_utf8(entity.id) {
                        for value in &entity.values {
                            if let Ok(property_id) = String::from_utf8(value.property_id.clone()) {
                                return Some(PropertyOp {
                                    id: derive_property_id(&entity_id, &property_id, space_id),
                                    change_type: PropertyChangeType::DELETE,
                                    property_id,
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
