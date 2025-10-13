use uuid::Uuid;
use crate::VoteCast;
use tracing::{instrument, warn};
use indexer_utils::{checksum_address, id::derive_space_id, network_ids::GEO};

#[derive(Debug, Clone)]
pub struct VoteItem {
    pub id: Uuid,
    pub onchain_proposal_id: String,
    pub voter_address: String,
    pub vote_option: i16,
    pub plugin_address: String,
    pub space_id: Uuid,
    pub proposal_id: Option<Uuid>,
}

pub struct VotesModel;

impl VotesModel {
    #[instrument(skip_all, fields(vote_count = votes.len(), block_number))]
    pub fn map_votes(
        votes: &[VoteCast],
        block_number: u64,
    ) -> Vec<VoteItem> {
        votes
            .iter()
            .filter_map(|vote| {
                // Generate deterministic UUID for the vote based on onchain_proposal_id and voter
                let vote_id = generate_vote_id(&vote.onchain_proposal_id, &vote.voter);
                
                // Validate DAO address format before attempting checksum
                if !is_valid_ethereum_address(&vote.dao_address) {
                    warn!(
                        dao_address = %vote.dao_address,
                        onchain_proposal_id = %vote.onchain_proposal_id,
                        voter = %vote.voter,
                        block_number = block_number,
                        "Invalid DAO address format for vote, skipping"
                    );
                    return None;
                }
                
                // Derive space_id from DAO address using the same logic as other models
                let checksummed_dao = checksum_address(vote.dao_address.clone());
                let space_id = derive_space_id(GEO, &checksummed_dao);
                
                // Try to parse the onchain_proposal_id as a proposal UUID
                let proposal_id = Uuid::parse_str(&vote.onchain_proposal_id).ok();
                
                Some(VoteItem {
                    id: vote_id,
                    onchain_proposal_id: vote.onchain_proposal_id.clone(),
                    voter_address: checksum_address(vote.voter.clone()),
                    vote_option: vote.vote_option as i16,
                    plugin_address: checksum_address(vote.plugin_address.clone()),
                    space_id,
                    proposal_id,
                })
            })
            .collect()
    }
}

/// Validate if a string is a valid Ethereum address format
fn is_valid_ethereum_address(address: &str) -> bool {
    let cleaned = address.to_lowercase().replace("0x", "");
    cleaned.len() == 40 && cleaned.chars().all(|c| c.is_ascii_hexdigit())
}

/// Generate a deterministic UUID for a vote based on proposal ID and voter address
fn generate_vote_id(onchain_proposal_id: &str, voter: &str) -> Uuid {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    
    let mut hasher = DefaultHasher::new();
    onchain_proposal_id.hash(&mut hasher);
    voter.hash(&mut hasher);
    
    let hash = hasher.finish();
    let bytes: [u8; 16] = [
        (hash >> 56) as u8,
        (hash >> 48) as u8,
        (hash >> 40) as u8,
        (hash >> 32) as u8,
        (hash >> 24) as u8,
        (hash >> 16) as u8,
        (hash >> 8) as u8,
        hash as u8,
        (hash >> 56) as u8,
        (hash >> 48) as u8,
        (hash >> 40) as u8,
        (hash >> 32) as u8,
        (hash >> 24) as u8,
        (hash >> 16) as u8,
        (hash >> 8) as u8,
        hash as u8,
    ];
    
    Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_vote_id_deterministic() {
        let proposal_id = "0x123abc";
        let voter1 = "0xvoter1";
        let voter2 = "0xvoter2";
        
        let id1 = generate_vote_id(proposal_id, voter1);
        let id2 = generate_vote_id(proposal_id, voter1);
        let id3 = generate_vote_id(proposal_id, voter2);
        
        assert_eq!(id1, id2, "Same inputs should generate same ID");
        assert_ne!(id1, id3, "Different voters should generate different IDs");
    }

    #[test]
    fn test_map_votes() {
        let dao_address1 = "0x1234567890123456789012345678901234567890".to_string();
        let dao_address2 = "0xabcdef1234567890123456789012345678901234".to_string();
        
        let votes = vec![
            VoteCast {
                onchain_proposal_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
                voter: "0x1234567890123456789012345678901234567890".to_string(),
                vote_option: 1,
                plugin_address: "0x1111111111111111111111111111111111111111".to_string(),
                dao_address: dao_address1.clone(),
            },
            VoteCast {
                onchain_proposal_id: "proposal123".to_string(),
                voter: "0xabcdef1234567890123456789012345678901234".to_string(),
                vote_option: 2,
                plugin_address: "0x2222222222222222222222222222222222222222".to_string(),
                dao_address: dao_address2.clone(),
            },
        ];

        let vote_items = VotesModel::map_votes(&votes, 12345);
        
        // Both votes should be mapped since we now derive space_id from DAO address
        assert_eq!(vote_items.len(), 2);
        
        let vote_item = &vote_items[0];
        assert_eq!(vote_item.onchain_proposal_id, "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(vote_item.voter_address, checksum_address("0x1234567890123456789012345678901234567890"));
        assert_eq!(vote_item.vote_option, 1);
        assert_eq!(vote_item.plugin_address, checksum_address("0x1111111111111111111111111111111111111111"));
        assert_eq!(vote_item.space_id, derive_space_id(GEO, &checksum_address(dao_address1)));
        assert!(vote_item.proposal_id.is_some());
    }
}