use indexer_utils::id;
use std::collections::HashMap;
use std::fmt;
use uuid::Uuid;
use wire::pb::grc20::{op::Payload, DataType as PbDataType, Edit};

// Constants for PostgreSQL enum values - must match schema.ts dataTypesEnum
pub const DATA_TYPE_TEXT: &str = "Text";
pub const DATA_TYPE_NUMBER: &str = "Number";
pub const DATA_TYPE_CHECKBOX: &str = "Checkbox";
pub const DATA_TYPE_TIME: &str = "Time";
pub const DATA_TYPE_POINT: &str = "Point";
pub const DATA_TYPE_RELATION: &str = "Relation";

// All valid data type enum values
pub const VALID_DATA_TYPE_VALUES: &[&str] = &[
    DATA_TYPE_TEXT,
    DATA_TYPE_NUMBER,
    DATA_TYPE_CHECKBOX,
    DATA_TYPE_TIME,
    DATA_TYPE_POINT,
    DATA_TYPE_RELATION,
];

/// Type-safe representation of data types
#[derive(Clone, Debug, PartialEq, Copy)]
pub enum DataType {
    Text,
    Number,
    Checkbox,
    Time,
    Point,
    Relation,
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataType::Text => write!(f, "{}", DATA_TYPE_TEXT),
            DataType::Number => write!(f, "{}", DATA_TYPE_NUMBER),
            DataType::Checkbox => write!(f, "{}", DATA_TYPE_CHECKBOX),
            DataType::Time => write!(f, "{}", DATA_TYPE_TIME),
            DataType::Point => write!(f, "{}", DATA_TYPE_POINT),
            DataType::Relation => write!(f, "{}", DATA_TYPE_RELATION),
        }
    }
}

impl AsRef<str> for DataType {
    fn as_ref(&self) -> &str {
        match self {
            DataType::Text => DATA_TYPE_TEXT,
            DataType::Number => DATA_TYPE_NUMBER,
            DataType::Checkbox => DATA_TYPE_CHECKBOX,
            DataType::Time => DATA_TYPE_TIME,
            DataType::Point => DATA_TYPE_POINT,
            DataType::Relation => DATA_TYPE_RELATION,
        }
    }
}

impl std::convert::TryFrom<&str> for DataType {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            DATA_TYPE_TEXT => Ok(DataType::Text),
            DATA_TYPE_NUMBER => Ok(DataType::Number),
            DATA_TYPE_CHECKBOX => Ok(DataType::Checkbox),
            DATA_TYPE_TIME => Ok(DataType::Time),
            DATA_TYPE_POINT => Ok(DataType::Point),
            DATA_TYPE_RELATION => Ok(DataType::Relation),
            _ => Err(format!("Unknown data type: {}", value)),
        }
    }
}

impl PartialEq<str> for DataType {
    fn eq(&self, other: &str) -> bool {
        self.as_ref() == other
    }
}

impl PartialEq<&str> for DataType {
    fn eq(&self, other: &&str) -> bool {
        self.as_ref() == *other
    }
}

impl DataType {
    /// Returns all valid DataType enum variants
    pub fn all_variants() -> Vec<DataType> {
        vec![
            DataType::Text,
            DataType::Number,
            DataType::Checkbox,
            DataType::Time,
            DataType::Point,
            DataType::Relation,
        ]
    }

    /// Returns all valid string representations of DataType enum variants
    pub fn all_string_values() -> &'static [&'static str] {
        VALID_DATA_TYPE_VALUES
    }

    /// Validates if a string is a valid DataType enum value
    pub fn is_valid_string(value: &str) -> bool {
        VALID_DATA_TYPE_VALUES.contains(&value)
    }
}

/// Represents a property with its ID and type information
#[derive(Clone, Debug)]
pub struct PropertyItem {
    pub id: Uuid,
    pub data_type: DataType,
}

pub struct PropertiesModel;

impl PropertiesModel {
    pub fn map_edit_to_properties(edit: &Edit) -> Vec<PropertyItem> {
        let mut properties: Vec<PropertyItem> = Vec::new();

        for op in &edit.ops {
            if let Some(payload) = &op.payload {
                if let Payload::CreateProperty(property) = payload {
                    let property_id_bytes = id::transform_id_bytes(property.id.clone());

                    match property_id_bytes {
                        Ok(property_id_bytes) => {
                            let property_id = Uuid::from_bytes(property_id_bytes);

                            if let Some(property_type) = native_type_to_data_type(property.data_type) {
                                properties.push(PropertyItem {
                                    id: property_id,
                                    data_type: property_type,
                                });
                            }
                        }
                        Err(_) => tracing::error!(
                            "[Properties][CreateProperty] Could not transform Vec<u8> for property.id {:?}",
                            &property.id
                        ),
                    }
                }
            }
        }

        // A single edit may have multiple CREATE property ops applied
        // to the same property id. We need to squash them down into a single
        // op so we can write to the db atomically using the final state of the ops.
        //
        // Ordering of these to-be-squashed ops matters. We use what the order is in
        // the edit.
        squash_properties(&properties)
    }
}

fn squash_properties(properties: &Vec<PropertyItem>) -> Vec<PropertyItem> {
    let mut hash = HashMap::new();

    for property in properties {
        hash.insert(property.id.clone(), property.clone());
    }

    let result: Vec<_> = hash.into_values().collect();

    return result;
}

fn native_type_to_data_type(native_type: i32) -> Option<DataType> {
    match PbDataType::try_from(native_type) {
        Ok(PbDataType::Text) => Some(DataType::Text),
        Ok(PbDataType::Number) => Some(DataType::Number),
        Ok(PbDataType::Checkbox) => Some(DataType::Checkbox),
        Ok(PbDataType::Time) => Some(DataType::Time),
        Ok(PbDataType::Point) => Some(DataType::Point),
        Ok(PbDataType::Relation) => Some(DataType::Relation),
        Err(_) => {
            tracing::error!("[Properties] Unknown native type: {}", native_type);
            None
        }
    }
}
