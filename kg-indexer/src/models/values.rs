use serde_json::Value as JsonValue;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub enum ValueChangeType {
    Set,
    Delete,
}

/// Value operation for GRC-20 v2.
///
/// Column mapping by DataType:
/// - Bool -> boolean
/// - Int64 -> integer
/// - Float64 -> float
/// - Decimal -> decimal (as string for precision)
/// - Text -> text
/// - Bytes -> bytes
/// - Date -> date
/// - Time -> time
/// - Datetime -> datetime
/// - Schedule -> schedule (jsonb)
/// - Point -> point
/// - Embedding -> embedding (jsonb)
#[derive(Clone, Debug)]
pub struct ValueOp {
    pub id: Uuid,
    pub change_type: ValueChangeType,
    pub entity_id: Uuid,
    pub property_id: Uuid,
    pub space_id: Uuid,
    pub language: Option<String>,
    pub unit: Option<String>,
    // Value columns (one will be set based on DataType)
    pub text: Option<String>,         // Text
    pub decimal: Option<String>,      // Decimal (stored as string for precision)
    pub boolean: Option<bool>,        // Bool
    pub time: Option<String>,         // Time
    pub point: Option<String>,        // Point
    pub integer: Option<i64>,         // Int64
    pub float: Option<f64>,           // Float64
    pub bytes: Option<Vec<u8>>,       // Bytes
    pub date: Option<String>,         // Date
    pub datetime: Option<String>,     // Datetime
    pub schedule: Option<JsonValue>,  // Schedule
    pub embedding: Option<JsonValue>, // Embedding
}
