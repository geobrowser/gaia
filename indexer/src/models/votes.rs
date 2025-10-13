use uuid::Uuid;
use crate::VoteCast;
use tracing::{instrument, warn};

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
                
                // Try to parse the dao_address as a space UUID
                match Uuid::parse_str(&vote.dao_address) {
                    Ok(space_id) => {
                        // Try to parse the onchain_proposal_id as a proposal UUID
                        let proposal_id = Uuid::parse_str(&vote.onchain_proposal_id).ok();
                        
                        Some(VoteItem {
                            id: vote_id,
                            onchain_proposal_id: vote.onchain_proposal_id.clone(),
                            voter_address: vote.voter.clone(),
                            vote_option: vote.vote_option as i16,
                            plugin_address: vote.plugin_address.clone(),
                            space_id,
                            proposal_id,
                        })
                    }
                    Err(e) => {
                        warn!(
                            dao_address = %vote.dao_address,
                            onchain_proposal_id = %vote.onchain_proposal_id,
                            voter = %vote.voter,
                            error = %e,
                            block_number = block_number,
                            "Failed to parse dao_address as UUID for vote, skipping"
                        );
                        None
                    }
                }
            })
            .collect()
    }
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
        let votes = vec![
            VoteCast {
                onchain_proposal_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
                voter: "0x1234567890123456789012345678901234567890".to_string(),
                vote_option: 1,
                plugin_address: "0xplugin123".to_string(),
                dao_address: "550e8400-e29b-41d4-a716-446655440001".to_string(), // Valid UUID
            },
            VoteCast {
                onchain_proposal_id: "proposal123".to_string(),
                voter: "0xabcdef".to_string(),
                vote_option: 2,
                plugin_address: "0xplugin456".to_string(),
                dao_address: "invalid-uuid".to_string(), // Invalid UUID
            },
        ];

        let vote_items = VotesModel::map_votes(&votes, 12345);
        
        // Only the first vote should be mapped (valid UUID for dao_address)
        assert_eq!(vote_items.len(), 1);
        
        let vote_item = &vote_items[0];
        assert_eq!(vote_item.onchain_proposal_id, "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(vote_item.voter_address, "0x1234567890123456789012345678901234567890");
        assert_eq!(vote_item.vote_option, 1);
        assert_eq!(vote_item.plugin_address, "0xplugin123");
        assert_eq!(vote_item.space_id.to_string(), "550e8400-e29b-41d4-a716-446655440001");
        assert!(vote_item.proposal_id.is_some());
    }
}