use indexer_utils::{checksum_address, id::derive_space_id, network_ids::GEO};
use uuid::Uuid;

use crate::{AddedMember, RemovedMember};

#[derive(Clone, Debug)]
pub struct MemberItem {
    pub address: String,
    pub space_id: Uuid,
}

#[derive(Clone, Debug)]
pub struct EditorItem {
    pub address: String,
    pub space_id: Uuid,
}

pub struct MembershipModel;

impl MembershipModel {
    /// Maps added members from KgData to database-ready MemberItem structs
    pub fn map_added_members(added_members: &Vec<AddedMember>) -> Vec<MemberItem> {
        let mut members = Vec::new();

        for member in added_members {
            let space_id = derive_space_id(GEO, &checksum_address(member.dao_address.clone()));
            
            members.push(MemberItem {
                address: checksum_address(member.editor_address.clone()),
                space_id,
            });
        }

        members
    }

    /// Maps removed members from KgData to database-ready MemberItem structs
    pub fn map_removed_members(removed_members: &Vec<RemovedMember>) -> Vec<MemberItem> {
        let mut members = Vec::new();

        for member in removed_members {
            let space_id = derive_space_id(GEO, &checksum_address(member.dao_address.clone()));
            
            members.push(MemberItem {
                address: checksum_address(member.editor_address.clone()),
                space_id,
            });
        }

        members
    }

    /// Maps added editors from KgData to database-ready EditorItem structs
    pub fn map_added_editors(added_editors: &Vec<AddedMember>) -> Vec<EditorItem> {
        let mut editors = Vec::new();

        for editor in added_editors {
            let space_id = derive_space_id(GEO, &checksum_address(editor.dao_address.clone()));
            
            editors.push(EditorItem {
                address: checksum_address(editor.editor_address.clone()),
                space_id,
            });
        }

        editors
    }

    /// Maps removed editors from KgData to database-ready EditorItem structs
    pub fn map_removed_editors(removed_editors: &Vec<RemovedMember>) -> Vec<EditorItem> {
        let mut editors = Vec::new();

        for editor in removed_editors {
            let space_id = derive_space_id(GEO, &checksum_address(editor.dao_address.clone()));
            
            editors.push(EditorItem {
                address: checksum_address(editor.editor_address.clone()),
                space_id,
            });
        }

        editors
    }
}