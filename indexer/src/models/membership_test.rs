#[cfg(test)]
mod tests {
    use crate::models::membership::MembershipModel;
    use crate::{AddedMember, RemovedMember};
    use indexer_utils::{checksum_address, id::derive_space_id, network_ids::GEO};

    fn create_added_member(dao_address: &str, editor_address: &str) -> AddedMember {
        AddedMember {
            dao_address: dao_address.to_string(),
            editor_address: editor_address.to_string(),
        }
    }

    fn create_removed_member(dao_address: &str, editor_address: &str) -> RemovedMember {
        RemovedMember {
            dao_address: dao_address.to_string(),
            editor_address: editor_address.to_string(),
        }
    }

    #[test]
    fn test_map_added_members_empty() {
        let added_members = vec![];
        let result = MembershipModel::map_added_members(&added_members);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_map_added_members_single() {
        let dao_addr = "0x1234567890123456789012345678901234567890";
        let editor_addr = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd1";
        
        let added_members = vec![create_added_member(dao_addr, editor_addr)];
        let result = MembershipModel::map_added_members(&added_members);
        
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].address, checksum_address(editor_addr.to_string()));
        assert_eq!(result[0].space_id, derive_space_id(GEO, &checksum_address(dao_addr.to_string())));
    }

    #[test]
    fn test_map_added_members_multiple() {
        let dao_addr1 = "0x1234567890123456789012345678901234567890";
        let dao_addr2 = "0x0987654321098765432109876543210987654321";
        let editor_addr1 = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd1";
        let editor_addr2 = "0xfedcbafedcbafedcbafedcbafedcbafedcbafed2";
        
        let added_members = vec![
            create_added_member(dao_addr1, editor_addr1),
            create_added_member(dao_addr2, editor_addr2),
        ];
        let result = MembershipModel::map_added_members(&added_members);
        
        assert_eq!(result.len(), 2);
        
        assert_eq!(result[0].address, checksum_address(editor_addr1.to_string()));
        assert_eq!(result[0].space_id, derive_space_id(GEO, &checksum_address(dao_addr1.to_string())));
        
        assert_eq!(result[1].address, checksum_address(editor_addr2.to_string()));
        assert_eq!(result[1].space_id, derive_space_id(GEO, &checksum_address(dao_addr2.to_string())));
    }

    #[test]
    fn test_map_added_members_same_space_different_members() {
        let dao_addr = "0x1234567890123456789012345678901234567890";
        let editor_addr1 = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd1";
        let editor_addr2 = "0xfedcbafedcbafedcbafedcbafedcbafedcbafed2";
        
        let added_members = vec![
            create_added_member(dao_addr, editor_addr1),
            create_added_member(dao_addr, editor_addr2),
        ];
        let result = MembershipModel::map_added_members(&added_members);
        
        assert_eq!(result.len(), 2);
        
        let expected_space_id = derive_space_id(GEO, &checksum_address(dao_addr.to_string()));
        assert_eq!(result[0].space_id, expected_space_id);
        assert_eq!(result[1].space_id, expected_space_id);
        
        assert_eq!(result[0].address, checksum_address(editor_addr1.to_string()));
        assert_eq!(result[1].address, checksum_address(editor_addr2.to_string()));
    }

    #[test]
    fn test_map_removed_members_empty() {
        let removed_members = vec![];
        let result = MembershipModel::map_removed_members(&removed_members);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_map_removed_members_single() {
        let dao_addr = "0x1234567890123456789012345678901234567890";
        let editor_addr = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd1";
        
        let removed_members = vec![create_removed_member(dao_addr, editor_addr)];
        let result = MembershipModel::map_removed_members(&removed_members);
        
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].address, checksum_address(editor_addr.to_string()));
        assert_eq!(result[0].space_id, derive_space_id(GEO, &checksum_address(dao_addr.to_string())));
    }

    #[test]
    fn test_map_removed_members_multiple() {
        let dao_addr1 = "0x1234567890123456789012345678901234567890";
        let dao_addr2 = "0x0987654321098765432109876543210987654321";
        let editor_addr1 = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd1";
        let editor_addr2 = "0xfedcbafedcbafedcbafedcbafedcbafedcbafed2";
        
        let removed_members = vec![
            create_removed_member(dao_addr1, editor_addr1),
            create_removed_member(dao_addr2, editor_addr2),
        ];
        let result = MembershipModel::map_removed_members(&removed_members);
        
        assert_eq!(result.len(), 2);
        
        assert_eq!(result[0].address, checksum_address(editor_addr1.to_string()));
        assert_eq!(result[0].space_id, derive_space_id(GEO, &checksum_address(dao_addr1.to_string())));
        
        assert_eq!(result[1].address, checksum_address(editor_addr2.to_string()));
        assert_eq!(result[1].space_id, derive_space_id(GEO, &checksum_address(dao_addr2.to_string())));
    }

    #[test]
    fn test_map_added_editors_empty() {
        let added_editors = vec![];
        let result = MembershipModel::map_added_editors(&added_editors);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_map_added_editors_single() {
        let dao_addr = "0x1234567890123456789012345678901234567890";
        let editor_addr = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd1";
        
        let added_editors = vec![create_added_member(dao_addr, editor_addr)];
        let result = MembershipModel::map_added_editors(&added_editors);
        
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].address, checksum_address(editor_addr.to_string()));
        assert_eq!(result[0].space_id, derive_space_id(GEO, &checksum_address(dao_addr.to_string())));
    }

    #[test]
    fn test_map_added_editors_multiple() {
        let dao_addr1 = "0x1234567890123456789012345678901234567890";
        let dao_addr2 = "0x0987654321098765432109876543210987654321";
        let editor_addr1 = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd1";
        let editor_addr2 = "0xfedcbafedcbafedcbafedcbafedcbafedcbafed2";
        
        let added_editors = vec![
            create_added_member(dao_addr1, editor_addr1),
            create_added_member(dao_addr2, editor_addr2),
        ];
        let result = MembershipModel::map_added_editors(&added_editors);
        
        assert_eq!(result.len(), 2);
        
        assert_eq!(result[0].address, checksum_address(editor_addr1.to_string()));
        assert_eq!(result[0].space_id, derive_space_id(GEO, &checksum_address(dao_addr1.to_string())));
        
        assert_eq!(result[1].address, checksum_address(editor_addr2.to_string()));
        assert_eq!(result[1].space_id, derive_space_id(GEO, &checksum_address(dao_addr2.to_string())));
    }

    #[test]
    fn test_map_added_editors_same_space_different_editors() {
        let dao_addr = "0x1234567890123456789012345678901234567890";
        let editor_addr1 = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd1";
        let editor_addr2 = "0xfedcbafedcbafedcbafedcbafedcbafedcbafed2";
        
        let added_editors = vec![
            create_added_member(dao_addr, editor_addr1),
            create_added_member(dao_addr, editor_addr2),
        ];
        let result = MembershipModel::map_added_editors(&added_editors);
        
        assert_eq!(result.len(), 2);
        
        let expected_space_id = derive_space_id(GEO, &checksum_address(dao_addr.to_string()));
        assert_eq!(result[0].space_id, expected_space_id);
        assert_eq!(result[1].space_id, expected_space_id);
        
        assert_eq!(result[0].address, checksum_address(editor_addr1.to_string()));
        assert_eq!(result[1].address, checksum_address(editor_addr2.to_string()));
    }

    #[test]
    fn test_map_removed_editors_empty() {
        let removed_editors = vec![];
        let result = MembershipModel::map_removed_editors(&removed_editors);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_map_removed_editors_single() {
        let dao_addr = "0x1234567890123456789012345678901234567890";
        let editor_addr = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd1";
        
        let removed_editors = vec![create_removed_member(dao_addr, editor_addr)];
        let result = MembershipModel::map_removed_editors(&removed_editors);
        
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].address, checksum_address(editor_addr.to_string()));
        assert_eq!(result[0].space_id, derive_space_id(GEO, &checksum_address(dao_addr.to_string())));
    }

    #[test]
    fn test_map_removed_editors_multiple() {
        let dao_addr1 = "0x1234567890123456789012345678901234567890";
        let dao_addr2 = "0x0987654321098765432109876543210987654321";
        let editor_addr1 = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd1";
        let editor_addr2 = "0xfedcbafedcbafedcbafedcbafedcbafedcbafed2";
        
        let removed_editors = vec![
            create_removed_member(dao_addr1, editor_addr1),
            create_removed_member(dao_addr2, editor_addr2),
        ];
        let result = MembershipModel::map_removed_editors(&removed_editors);
        
        assert_eq!(result.len(), 2);
        
        assert_eq!(result[0].address, checksum_address(editor_addr1.to_string()));
        assert_eq!(result[0].space_id, derive_space_id(GEO, &checksum_address(dao_addr1.to_string())));
        
        assert_eq!(result[1].address, checksum_address(editor_addr2.to_string()));
        assert_eq!(result[1].space_id, derive_space_id(GEO, &checksum_address(dao_addr2.to_string())));
    }

    #[test]
    fn test_address_checksumming() {
        let dao_addr = "0x1234567890abcdef1234567890abcdef12345678"; // lowercase
        let editor_addr = "0xABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABC1"; // uppercase
        
        let added_members = vec![create_added_member(dao_addr, editor_addr)];
        let result = MembershipModel::map_added_members(&added_members);
        
        assert_eq!(result.len(), 1);
        // Verify that addresses are properly checksummed
        assert_eq!(result[0].address, checksum_address(editor_addr.to_string()));
        assert_ne!(result[0].address, editor_addr); // Should be different if case was wrong
    }

    #[test]
    fn test_space_id_derivation_consistency() {
        let dao_addr = "0x1234567890123456789012345678901234567890";
        let editor_addr1 = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd1";
        let editor_addr2 = "0xfedcbafedcbafedcbafedcbafedcbafedcbafed2";
        
        // Test that members and editors for the same DAO get the same space_id
        let added_members = vec![create_added_member(dao_addr, editor_addr1)];
        let added_editors = vec![create_added_member(dao_addr, editor_addr2)];
        
        let member_result = MembershipModel::map_added_members(&added_members);
        let editor_result = MembershipModel::map_added_editors(&added_editors);
        
        assert_eq!(member_result[0].space_id, editor_result[0].space_id);
        
        // Verify it matches the expected derivation
        let expected_space_id = derive_space_id(GEO, &checksum_address(dao_addr.to_string()));
        assert_eq!(member_result[0].space_id, expected_space_id);
        assert_eq!(editor_result[0].space_id, expected_space_id);
    }
}