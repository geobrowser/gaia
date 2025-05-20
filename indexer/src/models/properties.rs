use std::collections::HashMap;

use grc20::pb::ipfsv2::{op::Payload, Edit, Op};

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
    pub language_option: Option<String>,
}

pub struct ValuesModel;

impl ValuesModel {
    pub fn map_edit_to_values(edit: &Edit, space_id: &String) -> (Vec<ValueOp>, Vec<String>) {
        let mut triple_ops: Vec<ValueOp> = Vec::new();

        for op in &edit.ops {
            if let Some(triple_op) = value_op_from_op(op, space_id) {
                triple_ops.push(triple_op);
            }
        }

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

fn value_op_from_op(op: &Op, space_id: &String) -> Option<ValueOp> {
    if let Some(payload) = &op.payload {
        return match payload {
            Payload::CreateEntity(entity) | Payload::UpdateEntity(entity) => {
                if let Ok(entity_id) = String::from_utf8(entity.id.clone()) {
                    for value in &entity.values {
                        let property_id = String::from_utf8(value.property_id.clone());

                        if let Ok(property_id) = property_id {
                            return Some(ValueOp {
                                id: derive_value_id(&entity_id, &property_id, space_id),
                                change_type: ValueChangeType::SET,
                                property_id,
                                entity_id,
                                space_id: space_id.clone(),
                                value: Some(value.value.clone()),
                                language_option: None,
                            });
                        }
                    }
                }

                return None;
            }
            Payload::UnsetProperties(entity) => {
                if let Ok(entity_id) = String::from_utf8(entity.id.clone()) {
                    for property in &entity.properties {
                        if let Ok(property_id) = String::from_utf8(property.clone()) {
                            return Some(ValueOp {
                                id: derive_value_id(&entity_id, &property_id, space_id),
                                change_type: ValueChangeType::DELETE,
                                property_id,
                                entity_id,
                                space_id: space_id.clone(),
                                value: None,
                                language_option: None,
                            });
                        }
                    }
                }

                return None;
            }
            _ => None,
        };
    }

    None
}
