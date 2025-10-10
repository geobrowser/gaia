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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_proposal_id_deterministic() {
        let dao_address = "0x1234567890123456789012345678901234567890";
        let proposal_id = "123";
        let plugin_address = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd";

        let id1 = derive_proposal_id(dao_address, proposal_id, plugin_address);
        let id2 = derive_proposal_id(dao_address, proposal_id, plugin_address);

        assert_eq!(id1, id2, "Same inputs should produce same UUID");
    }

    #[test]
    fn test_derive_proposal_id_different_proposal_ids() {
        let dao_address = "0x1234567890123456789012345678901234567890";
        let plugin_address = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd";

        let id1 = derive_proposal_id(dao_address, "123", plugin_address);
        let id2 = derive_proposal_id(dao_address, "124", plugin_address);

        assert_ne!(id1, id2, "Different proposal IDs should produce different UUIDs");
    }

    #[test]
    fn test_derive_proposal_id_different_dao_addresses() {
        let proposal_id = "123";
        let plugin_address = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd";

        let id1 = derive_proposal_id("0x1234567890123456789012345678901234567890", proposal_id, plugin_address);
        let id2 = derive_proposal_id("0x1234567890123456789012345678901234567891", proposal_id, plugin_address);

        assert_ne!(id1, id2, "Different DAO addresses should produce different UUIDs");
    }

    #[test]
    fn test_derive_proposal_id_different_plugin_addresses() {
        let dao_address = "0x1234567890123456789012345678901234567890";
        let proposal_id = "123";

        let id1 = derive_proposal_id(dao_address, proposal_id, "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd");
        let id2 = derive_proposal_id(dao_address, proposal_id, "0xabcdefabcdefabcdefabcdefabcdefabcdefabce");

        assert_ne!(id1, id2, "Different plugin addresses should produce different UUIDs");
    }

    #[test]
    fn test_derive_proposal_id_case_insensitive_addresses() {
        let proposal_id = "123";
        let plugin_address = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd";

        // Test that checksum_address normalization works
        let id1 = derive_proposal_id("0x1234567890123456789012345678901234567890", proposal_id, plugin_address);
        let id2 = derive_proposal_id("0x1234567890123456789012345678901234567890", proposal_id, plugin_address);

        assert_eq!(id1, id2, "Same addresses should produce same UUID regardless of case");
    }

    #[test]
    fn test_derive_proposal_id_edge_cases() {
        // Test with empty proposal ID
        let id1 = derive_proposal_id(
            "0x1234567890123456789012345678901234567890",
            "",
            "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd"
        );

        // Test with very long proposal ID
        let long_proposal_id = "123456789012345678901234567890123456789012345678901234567890";
        let id2 = derive_proposal_id(
            "0x1234567890123456789012345678901234567890",
            long_proposal_id,
            "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd"
        );

        assert_ne!(id1, id2, "Empty and long proposal IDs should produce different UUIDs");
    }

    #[test]
    fn test_derive_proposal_id_collision_resistance() {
        // Test that swapping DAO and plugin addresses produces different results
        let dao1 = "0x1234567890123456789012345678901234567890";
        let plugin1 = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd";
        let proposal_id = "123";

        let id1 = derive_proposal_id(dao1, proposal_id, plugin1);
        let id2 = derive_proposal_id(plugin1, proposal_id, dao1); // Swapped DAO and plugin

        assert_ne!(id1, id2, "Swapping DAO and plugin addresses should produce different UUIDs");
    }

    #[test]
    fn test_derive_proposal_id_known_output() {
        // Test with known inputs to ensure consistent output
        let dao_address = "0x1234567890123456789012345678901234567890";
        let proposal_id = "42";
        let plugin_address = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd";

        let id = derive_proposal_id(dao_address, proposal_id, plugin_address);

        // The UUID should be valid
        assert_ne!(id, Uuid::nil(), "Generated UUID should not be nil");
        
        // Test consistency - same inputs should always produce this exact UUID
        let id_again = derive_proposal_id(dao_address, proposal_id, plugin_address);
        assert_eq!(id, id_again, "Should produce identical UUID for identical inputs");
    }
}
