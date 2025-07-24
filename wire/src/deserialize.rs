use crate::pb::grc20::Edit;
use prost::Message;

pub fn deserialize(buf: &[u8]) -> std::result::Result<Edit, prost::DecodeError> {
    Edit::decode(buf)
}
