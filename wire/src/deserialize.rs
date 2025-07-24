use crate::pb::grc20::Edit;
use prost::Message;
use serde_json;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DeserializeError {
    #[error("JSON deserialization error: {0}")]
    JsonDeserializeError(#[from] serde_json::Error),

    #[error("Protobuf deserialization error: {0}")]
    ProtobufDeserializeError(#[from] prost::DecodeError),
}

pub fn deserialize(buf: &[u8]) -> Result<Edit, DeserializeError> {
    Ok(Edit::decode(buf)?)
}

pub fn deserialize_from_json(json: serde_json::Value) -> Result<Edit, DeserializeError> {
    Ok(serde_json::from_value::<Edit>(json)?)
}
