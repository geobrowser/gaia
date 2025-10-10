use indexer_utils::{checksum_address, id::derive_space_id, network_ids::GEO};
use uuid::Uuid;

use crate::{ProposalCreated, ExecutedProposal};

#[derive(Clone, Debug)]
pub enum ProposalType {
    PublishEdit,
    AddMember,
    RemoveMember,
    AddEditor,
    RemoveEditor,
    AddSubspace,
    RemoveSubspace,
}

#[derive(Clone, Debug)]
pub enum ProposalStatus {
    Created,
    Executed,
    Failed,
    Expired,
}

#[derive(Clone, Debug)]
pub struct ProposalItem {
    pub id: Uuid,
    pub space_id: Uuid,
    pub proposal_type: ProposalType,
    pub creator: String,
    pub start_time: i64,
    pub end_time: i64,
    pub status: ProposalStatus,
    pub content_uri: Option<String>,
    pub address: Option<String>,
    pub created_at_block: i64,
}

pub struct ProposalsModel;

impl ProposalsModel {
    pub fn map_created_proposals(
        proposals: &Vec<ProposalCreated>,
        block_number: i64,
    ) -> Vec<ProposalItem> {
        let mut proposal_items = Vec::new();

        for proposal in proposals {
            let proposal_item = match proposal {
                ProposalCreated::PublishEdit {
                    proposal_id,
                    creator,
                    start_time,
                    end_time,
                    content_uri,
                    dao_address,
                    edit_id,
                    ..
                } => {
                    let space_id = derive_space_id(GEO, &checksum_address(dao_address.clone()));
                    
                    // Use the Edit ID if available, otherwise use the proposal ID
                    let id = edit_id.unwrap_or_else(|| {
                        Uuid::parse_str(proposal_id).unwrap_or_else(|_| Uuid::new_v4())
                    });
                    
                    ProposalItem {
                        id,
                        space_id,
                        proposal_type: ProposalType::PublishEdit,
                        creator: checksum_address(creator.clone()),
                        start_time: start_time.parse().unwrap_or(0),
                        end_time: end_time.parse().unwrap_or(0),
                        status: ProposalStatus::Created,
                        content_uri: Some(content_uri.clone()),
                        address: None,
                        created_at_block: block_number,
                    }
                }
                ProposalCreated::AddMember {
                    proposal_id,
                    creator,
                    start_time,
                    end_time,
                    member,
                    dao_address,
                    ..
                } => {
                    let space_id = derive_space_id(GEO, &checksum_address(dao_address.clone()));
                    
                    ProposalItem {
                        id: Uuid::parse_str(proposal_id).unwrap_or_else(|_| Uuid::new_v4()),
                        space_id,
                        proposal_type: ProposalType::AddMember,
                        creator: checksum_address(creator.clone()),
                        start_time: start_time.parse().unwrap_or(0),
                        end_time: end_time.parse().unwrap_or(0),
                        status: ProposalStatus::Created,
                        content_uri: None,
                        address: Some(checksum_address(member.clone())),
                        created_at_block: block_number,
                    }
                }
                ProposalCreated::RemoveMember {
                    proposal_id,
                    creator,
                    start_time,
                    end_time,
                    member,
                    dao_address,
                    ..
                } => {
                    let space_id = derive_space_id(GEO, &checksum_address(dao_address.clone()));
                    
                    ProposalItem {
                        id: Uuid::parse_str(proposal_id).unwrap_or_else(|_| Uuid::new_v4()),
                        space_id,
                        proposal_type: ProposalType::RemoveMember,
                        creator: checksum_address(creator.clone()),
                        start_time: start_time.parse().unwrap_or(0),
                        end_time: end_time.parse().unwrap_or(0),
                        status: ProposalStatus::Created,
                        content_uri: None,
                        address: Some(checksum_address(member.clone())),
                        created_at_block: block_number,
                    }
                }
                ProposalCreated::AddEditor {
                    proposal_id,
                    creator,
                    start_time,
                    end_time,
                    editor,
                    dao_address,
                    ..
                } => {
                    let space_id = derive_space_id(GEO, &checksum_address(dao_address.clone()));
                    
                    ProposalItem {
                        id: Uuid::parse_str(proposal_id).unwrap_or_else(|_| Uuid::new_v4()),
                        space_id,
                        proposal_type: ProposalType::AddEditor,
                        creator: checksum_address(creator.clone()),
                        start_time: start_time.parse().unwrap_or(0),
                        end_time: end_time.parse().unwrap_or(0),
                        status: ProposalStatus::Created,
                        content_uri: None,
                        address: Some(checksum_address(editor.clone())),
                        created_at_block: block_number,
                    }
                }
                ProposalCreated::RemoveEditor {
                    proposal_id,
                    creator,
                    start_time,
                    end_time,
                    editor,
                    dao_address,
                    ..
                } => {
                    let space_id = derive_space_id(GEO, &checksum_address(dao_address.clone()));
                    
                    ProposalItem {
                        id: Uuid::parse_str(proposal_id).unwrap_or_else(|_| Uuid::new_v4()),
                        space_id,
                        proposal_type: ProposalType::RemoveEditor,
                        creator: checksum_address(creator.clone()),
                        start_time: start_time.parse().unwrap_or(0),
                        end_time: end_time.parse().unwrap_or(0),
                        status: ProposalStatus::Created,
                        content_uri: None,
                        address: Some(checksum_address(editor.clone())),
                        created_at_block: block_number,
                    }
                }
                ProposalCreated::AddSubspace {
                    proposal_id,
                    creator,
                    start_time,
                    end_time,
                    subspace,
                    dao_address,
                    ..
                } => {
                    let space_id = derive_space_id(GEO, &checksum_address(dao_address.clone()));
                    
                    ProposalItem {
                        id: Uuid::parse_str(proposal_id).unwrap_or_else(|_| Uuid::new_v4()),
                        space_id,
                        proposal_type: ProposalType::AddSubspace,
                        creator: checksum_address(creator.clone()),
                        start_time: start_time.parse().unwrap_or(0),
                        end_time: end_time.parse().unwrap_or(0),
                        status: ProposalStatus::Created,
                        content_uri: None,
                        address: Some(checksum_address(subspace.clone())),
                        created_at_block: block_number,
                    }
                }
                ProposalCreated::RemoveSubspace {
                    proposal_id,
                    creator,
                    start_time,
                    end_time,
                    subspace,
                    dao_address,
                    ..
                } => {
                    let space_id = derive_space_id(GEO, &checksum_address(dao_address.clone()));
                    
                    ProposalItem {
                        id: Uuid::parse_str(proposal_id).unwrap_or_else(|_| Uuid::new_v4()),
                        space_id,
                        proposal_type: ProposalType::RemoveSubspace,
                        creator: checksum_address(creator.clone()),
                        start_time: start_time.parse().unwrap_or(0),
                        end_time: end_time.parse().unwrap_or(0),
                        status: ProposalStatus::Created,
                        content_uri: None,
                        address: Some(checksum_address(subspace.clone())),
                        created_at_block: block_number,
                    }
                }
            };

            proposal_items.push(proposal_item);
        }

        proposal_items
    }

    pub fn map_executed_proposals(executed_proposals: &Vec<ExecutedProposal>) -> Vec<Uuid> {
        executed_proposals
            .iter()
            .filter_map(|ep| Uuid::parse_str(&ep.proposal_id).ok())
            .collect()
    }
}