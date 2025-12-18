use std::fmt;
use uuid::Uuid;

pub const DATA_TYPE_STRING: &str = "String";
pub const DATA_TYPE_NUMBER: &str = "Number";
pub const DATA_TYPE_BOOLEAN: &str = "Boolean";
pub const DATA_TYPE_TIME: &str = "Time";
pub const DATA_TYPE_POINT: &str = "Point";
pub const DATA_TYPE_RELATION: &str = "Relation";

#[derive(Clone, Debug, PartialEq, Copy)]
pub enum DataType {
    String,
    Number,
    Boolean,
    Time,
    Point,
    Relation,
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataType::String => write!(f, "{}", DATA_TYPE_STRING),
            DataType::Number => write!(f, "{}", DATA_TYPE_NUMBER),
            DataType::Boolean => write!(f, "{}", DATA_TYPE_BOOLEAN),
            DataType::Time => write!(f, "{}", DATA_TYPE_TIME),
            DataType::Point => write!(f, "{}", DATA_TYPE_POINT),
            DataType::Relation => write!(f, "{}", DATA_TYPE_RELATION),
        }
    }
}

impl AsRef<str> for DataType {
    fn as_ref(&self) -> &str {
        match self {
            DataType::String => DATA_TYPE_STRING,
            DataType::Number => DATA_TYPE_NUMBER,
            DataType::Boolean => DATA_TYPE_BOOLEAN,
            DataType::Time => DATA_TYPE_TIME,
            DataType::Point => DATA_TYPE_POINT,
            DataType::Relation => DATA_TYPE_RELATION,
        }
    }
}

impl std::convert::TryFrom<i32> for DataType {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        use wire::pb::grc20::DataType as PbDataType;
        match PbDataType::try_from(value) {
            Ok(PbDataType::String) => Ok(DataType::String),
            Ok(PbDataType::Number) => Ok(DataType::Number),
            Ok(PbDataType::Boolean) => Ok(DataType::Boolean),
            Ok(PbDataType::Time) => Ok(DataType::Time),
            Ok(PbDataType::Point) => Ok(DataType::Point),
            Ok(PbDataType::Relation) => Ok(DataType::Relation),
            Err(_) => Err(format!("Unknown data type: {}", value)),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PropertyItem {
    pub id: Uuid,
    pub data_type: DataType,
}
