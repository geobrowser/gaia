use prost::Message;

pub fn deserialize<T: Message + Default>(buf: &[u8]) -> Result<T, prost::DecodeError> {
    T::decode(buf)
}
