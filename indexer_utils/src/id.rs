use md5::{Digest, Md5};
use uuid::{Builder, Uuid};

use crate::checksum_address;

pub fn derive_space_id(network: &str, dao_address: &str) -> Uuid {
    let mut hasher = Md5::new();
    hasher.update(format!("{}:{}", network, checksum_address(dao_address)));
    let hashed: [u8; 16] = hasher.finalize().into();

    Builder::from_random_bytes(hashed).into_uuid()
}

pub fn derive_proposal_id(
    dao_address: &str,
    proposal_id: &str,
    plugin_address: &str,
) -> Uuid {
    let mut hasher = Md5::new();
    hasher.update(format!(
        "{}:{}:{}",
        checksum_address(dao_address),
        proposal_id,
        checksum_address(plugin_address)
    ));
    let hashed: [u8; 16] = hasher.finalize().into();

    Builder::from_random_bytes(hashed).into_uuid()
}
#[derive(Clone, Debug)]
pub enum IdError {
    DecodeError,
}

pub fn transform_id_bytes(bytes: Vec<u8>) -> Result<[u8; 16], IdError> {
    match bytes.try_into() {
        Ok(value) => Ok(value),
        Err(_) => Err(IdError::DecodeError),
    }
}
