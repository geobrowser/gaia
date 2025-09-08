use indexer_utils::{checksum_address, id::derive_space_id, network_ids::GEO};
use uuid::Uuid;

use crate::{AddedSubspace, RemovedSubspace};

#[derive(Clone, Debug)]
pub struct SubspaceItem {
    pub subspace_id: Uuid,
    pub parent_space_id: Uuid,
}

pub struct SubspaceModel;

impl SubspaceModel {
    /// Maps added subspaces from KgData to database-ready SubspaceItem structs
    pub fn map_added_subspaces(added_subspaces: &Vec<AddedSubspace>) -> Vec<SubspaceItem> {
        let mut subspaces = Vec::new();

        for subspace in added_subspaces {
            let parent_space_id =
                derive_space_id(GEO, &checksum_address(subspace.dao_address.clone()));
            let subspace_id =
                derive_space_id(GEO, &checksum_address(subspace.subspace_address.clone()));

            subspaces.push(SubspaceItem {
                subspace_id,
                parent_space_id,
            });
        }

        subspaces
    }

    /// Maps removed subspaces from KgData to database-ready SubspaceItem structs
    pub fn map_removed_subspaces(removed_subspaces: &Vec<RemovedSubspace>) -> Vec<SubspaceItem> {
        let mut subspaces = Vec::new();

        for subspace in removed_subspaces {
            let parent_space_id =
                derive_space_id(GEO, &checksum_address(subspace.dao_address.clone()));
            let subspace_id =
                derive_space_id(GEO, &checksum_address(subspace.subspace_address.clone()));

            subspaces.push(SubspaceItem {
                subspace_id,
                parent_space_id,
            });
        }

        subspaces
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_added_subspaces_empty() {
        let added_subspaces = vec![];
        let result = SubspaceModel::map_added_subspaces(&added_subspaces);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_map_added_subspaces() {
        let added_subspaces = vec![
            AddedSubspace {
                dao_address: "0x1234567890abcdef1234567890abcdef12345678".to_string(),
                subspace_address: "0xfedcba0987654321fedcba0987654321fedcba09".to_string(),
            },
            AddedSubspace {
                dao_address: "0xaabbccddee112233aabbccddee112233aabbccdd".to_string(),
                subspace_address: "0x9988776655443322998877665544332299887766".to_string(),
            },
        ];

        let result = SubspaceModel::map_added_subspaces(&added_subspaces);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_map_removed_subspaces_empty() {
        let removed_subspaces = vec![];
        let result = SubspaceModel::map_removed_subspaces(&removed_subspaces);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_map_removed_subspaces() {
        let removed_subspaces = vec![RemovedSubspace {
            dao_address: "0x1234567890abcdef1234567890abcdef12345678".to_string(),
            subspace_address: "0xfedcba0987654321fedcba0987654321fedcba09".to_string(),
        }];

        let result = SubspaceModel::map_removed_subspaces(&removed_subspaces);
        assert_eq!(result.len(), 1);
    }
}
