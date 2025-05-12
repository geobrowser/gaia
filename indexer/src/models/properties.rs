use std::collections::HashMap;

use grc20::pb::ipfs::{Edit, Op, OpType, Options, Value, ValueType as PbValueType};

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
    pub value_type: PbValueType, // @TODO: This gets moved to property eventually
    pub text_value: Option<String>,
    pub boolean_value: Option<bool>,
    pub number_value: Option<String>, // This just gets stored as text?
    pub language_option: Option<String>,
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
        let validated = validate_properties(&squashed);
        let (created, deleted): (Vec<PropertyOp>, Vec<PropertyOp>) = validated
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

fn validate_properties(triple_ops: &Vec<PropertyOp>) -> Vec<PropertyOp> {
    let validated = triple_ops
        .iter()
        .filter(|op| match op.change_type {
            PropertyChangeType::DELETE => true,
            // Verify that for each value type we have set the correct property
            // on the triple.
            //
            // Currently everything uses text_value except checkboxes
            PropertyChangeType::SET => match op.value_type {
                PbValueType::Checkbox => return op.boolean_value.is_some(),
                _ => return op.text_value.is_some(),
            },
        })
        .cloned()
        .collect();

    return validated;
}

fn derive_property_id(entity_id: &String, attribute_id: &String, space_id: &String) -> String {
    format!("{}:{}:{}", entity_id, attribute_id, space_id)
}

fn property_op_from_op(op: &Op, space_id: &String) -> Option<PropertyOp> {
    if let Ok(op_type) = OpType::try_from(op.r#type) {
        return match op_type {
            // SET_TRIPLE
            OpType::SetTriple => {
                if let Some(triple) = op.triple.clone() {
                    // @TODO: How do we map the value to the right place based on value_type?
                    if let Some(value) = triple.value {
                        let triple_values = map_property_value(&value).unwrap();
                        let triple_value_options = &value.options.unwrap_or(Options {
                            format: None,
                            language: None,
                            unit: None,
                        });

                        return Some(PropertyOp {
                            id: derive_property_id(&triple.entity, &triple.attribute, space_id),
                            change_type: PropertyChangeType::SET,
                            attribute_id: triple.attribute,
                            entity_id: triple.entity,
                            space_id: space_id.clone(),
                            value_type: triple_values.value_type,
                            text_value: triple_values.text_value,
                            number_value: triple_values.number_value,
                            boolean_value: triple_values.boolean_value,
                            unit_option: triple_value_options.unit.clone(),
                            format_option: triple_value_options.format.clone(),
                            language_option: triple_value_options.language.clone(),
                        });
                    }
                }

                return None;
            }
            OpType::DeleteTriple => {
                if let Some(triple) = op.triple.clone() {
                    return Some(PropertyOp {
                        id: derive_property_id(&triple.entity, &triple.attribute, space_id),
                        change_type: PropertyChangeType::DELETE,
                        attribute_id: triple.attribute,
                        entity_id: triple.entity,
                        space_id: space_id.clone(),
                        text_value: None,
                        number_value: None,
                        unit_option: None,
                        boolean_value: None,
                        format_option: None,
                        language_option: None,

                        // It doesn't matter what value type we use here since it's being deleted
                        value_type: PbValueType::Text,
                    });
                }

                return None;
            }
            _ => None,
        };
    };

    None
}

struct TripleValues {
    value_type: PbValueType,
    text_value: Option<String>,
    number_value: Option<String>,
    boolean_value: Option<bool>,
}

fn map_property_value(value: &Value) -> Option<TripleValues> {
    if let Ok(value_type) = PbValueType::try_from(value.r#type) {
        let value = value.value.clone();

        return match value_type {
            PbValueType::Text => Some(TripleValues {
                value_type,
                boolean_value: None,
                number_value: None,
                text_value: Some(value),
            }),
            PbValueType::Checkbox => {
                let maybe_bool_value = match value.as_str() {
                    "0" => Some(false),
                    "1" => Some(true),
                    _ => None,
                };

                Some(TripleValues {
                    value_type,
                    boolean_value: maybe_bool_value,
                    number_value: None,
                    text_value: None,
                })
            }
            PbValueType::Number => Some(TripleValues {
                value_type,
                boolean_value: None,
                number_value: None,
                text_value: Some(value),
            }),
            PbValueType::Point => Some(TripleValues {
                value_type,
                boolean_value: None,
                number_value: None,
                text_value: Some(value),
            }),
            PbValueType::Time => Some(TripleValues {
                value_type,
                boolean_value: None,
                number_value: None,
                text_value: Some(value),
            }),
            PbValueType::Url => Some(TripleValues {
                value_type,
                boolean_value: None,
                number_value: None,
                text_value: Some(value),
            }),
            PbValueType::Unknown => None,
        };
    }

    None
}

impl std::fmt::Display for PropertyOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Format id, change_type, and the triple identifiers
        write!(
            f,
            "Triple[{}] {{ {}, {}, {}, {} }}",
            self.id, self.entity_id, self.attribute_id, self.space_id, self.change_type
        )?;

        // Output the correct value based on value_type
        match self.value_type {
            PbValueType::Text => {
                if let Some(text) = &self.text_value {
                    write!(f, "\"{}\"", text)?;
                } else {
                    write!(f, "null")?;
                }
            }
            PbValueType::Checkbox => {
                if let Some(boolean) = self.boolean_value {
                    write!(f, "{}", boolean)?;
                } else {
                    write!(f, "null")?;
                }
            }
            PbValueType::Number => {
                if let Some(number) = &self.number_value {
                    write!(f, "{}", number)?;
                } else {
                    write!(f, "null")?;
                }
            }
            _ => write!(f, "unknown")?,
        }

        // Format optional metadata as key-value pairs if they exist
        let mut options = Vec::new();

        if let Some(lang) = &self.language_option {
            if !lang.is_empty() {
                options.push(format!("lang={}", lang));
            }
        }

        if let Some(format) = &self.format_option {
            if !format.is_empty() {
                options.push(format!("format={}", format));
            }
        }

        if let Some(unit) = &self.unit_option {
            if !unit.is_empty() {
                options.push(format!("unit={}", unit));
            }
        }

        // Add options if there are any
        if !options.is_empty() {
            write!(f, " ({})", options.join(", "))?;
        }

        Ok(())
    }
}

impl std::fmt::Display for PropertyChangeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PropertyChangeType::SET => write!(f, "SET"),
            PropertyChangeType::DELETE => write!(f, "DELETE"),
        }
    }
}
