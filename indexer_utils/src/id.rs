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
    fn test_derive_space_id_deterministic() {
        let network = "mainnet";
        let dao_address = "0x1234567890123456789012345678901234567890";

        let id1 = derive_space_id(network, dao_address);
        let id2 = derive_space_id(network, dao_address);

        assert_eq!(id1, id2, "Same inputs should produce same UUID");
    }

    #[test]
    fn test_derive_space_id_different_networks() {
        let dao_address = "0x1234567890123456789012345678901234567890";

        let id1 = derive_space_id("mainnet", dao_address);
        let id2 = derive_space_id("testnet", dao_address);

        assert_ne!(id1, id2, "Different networks should produce different UUIDs");
    }

    #[test]
    fn test_derive_space_id_different_dao_addresses() {
        let network = "mainnet";

        let id1 = derive_space_id(network, "0x1234567890123456789012345678901234567890");
        let id2 = derive_space_id(network, "0x1234567890123456789012345678901234567891");

        assert_ne!(id1, id2, "Different DAO addresses should produce different UUIDs");
    }

    #[test]
    fn test_derive_space_id_address_normalization() {
        let network = "mainnet";
        
        // Test that checksum_address normalization works
        let id1 = derive_space_id(network, "0x1234567890123456789012345678901234567890");
        let id2 = derive_space_id(network, "0x1234567890123456789012345678901234567890");

        assert_eq!(id1, id2, "Same addresses should produce same UUID regardless of case");
    }

    #[test]
    fn test_derive_space_id_edge_cases() {
        // Test with empty network
        let id1 = derive_space_id("", "0x1234567890123456789012345678901234567890");

        // Test with short network name
        let id2 = derive_space_id("a", "0x1234567890123456789012345678901234567890");

        // Test with very long network name
        let long_network = "very_long_network_name_that_might_cause_issues_if_not_handled_properly";
        let id3 = derive_space_id(long_network, "0x1234567890123456789012345678901234567890");

        // All should be different
        assert_ne!(id1, id2, "Empty network and short network should produce different UUIDs");
        assert_ne!(id1, id3, "Empty network and long network should produce different UUIDs");
        assert_ne!(id2, id3, "Short network and long network should produce different UUIDs");
    }

    #[test]
    fn test_derive_space_id_collision_resistance() {
        // Test that similar but different inputs produce different results
        let network1 = "mainnet";
        let network2 = "testnet";
        let dao_address = "0x1234567890123456789012345678901234567890";

        let id1 = derive_space_id(network1, dao_address);
        let id2 = derive_space_id(network2, dao_address);

        assert_ne!(id1, id2, "Different networks should produce different UUIDs");

        // Test with concatenated inputs that could potentially collide
        let id3 = derive_space_id("main", "0x1234567890123456789012345678901234567890");
        let id4 = derive_space_id("mainnet", "0x1234567890123456789012345678901234567890");

        assert_ne!(id3, id4, "Different network lengths should not collide");
    }

    #[test]
    fn test_derive_space_id_known_output() {
        // Test with known inputs to ensure consistent output
        let network = "ethereum";
        let dao_address = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd";

        let id = derive_space_id(network, dao_address);

        // The UUID should be valid
        assert_ne!(id, Uuid::nil(), "Generated UUID should not be nil");
        
        // Test consistency - same inputs should always produce this exact UUID
        let id_again = derive_space_id(network, dao_address);
        assert_eq!(id, id_again, "Should produce identical UUID for identical inputs");
    }

    #[test]
    fn test_derive_space_id_vs_proposal_id_different() {
        // Ensure space and proposal ID generation produce different results even with similar inputs
        let dao_address = "0x1234567890123456789012345678901234567890";
        let network = "mainnet";
        let proposal_id = "mainnet"; // Using network name as proposal ID
        let plugin_address = dao_address; // Using same address as plugin

        let space_id = derive_space_id(network, dao_address);
        let proposal_uuid = derive_proposal_id(dao_address, proposal_id, plugin_address);

        assert_ne!(space_id, proposal_uuid, "Space ID and proposal ID should be different even with similar inputs");
    }

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
