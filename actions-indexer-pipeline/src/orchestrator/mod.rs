//! This module defines the `Orchestrator` responsible for coordinating the
//! action processing pipeline.
//! It integrates the consumer, processor, and loader components to manage the
//! flow of action events from ingestion to persistence.
use crate::errors::OrchestratorError;
use crate::consumer::{ActionsConsumer, StreamMessage};
use crate::processor::{ActionsProcessor, ProcessActions};
use crate::loader::ActionsLoader;
use actions_indexer_shared::types::{Action, Changeset, UserVote, Vote, VoteCriteria, VoteCountCriteria, VoteValue, VotesCount};
use tokio::sync::mpsc;
use std::collections::HashMap;
use actions_indexer_repository::{ActionsRepository, CursorRepository};

/// `Orchestrator` is responsible for coordinating the consumption, processing,
/// and loading of actions.
///
/// It holds references to the `ConsumeActions`, `ProcessActions`, and
/// `ActionsLoader` traits, enabling a flexible and extensible pipeline.
pub struct Orchestrator {
    pub actions_consumer: Box<ActionsConsumer>,
    pub actions_processor: Box<ActionsProcessor>,
    pub actions_loader: Box<ActionsLoader>,
}

impl Orchestrator {
    /// Creates a new `Orchestrator` instance.
    ///
    /// # Arguments
    ///
    /// * `actions_consumer` - A boxed `ActionsConsumer` instance
    /// * `actions_processor` - A boxed `ActionsProcessor` instance
    /// * `actions_loader` - A boxed `ActionsLoader` instance
    ///
    /// # Returns
    ///
    /// A new `Orchestrator` instance.
    pub fn new(
        actions_consumer: Box<ActionsConsumer>,
        actions_processor: Box<ActionsProcessor>,
        actions_loader: Box<ActionsLoader>,
    ) -> Self {
        Self {
            actions_consumer,
            actions_processor,
            actions_loader,
        }
    }

    /// Runs the orchestrator, initiating the action processing pipeline.
    ///
    /// This method is the main entry point for starting the continuous flow of
    /// action consumption, processing, and loading.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or an `OrchestratorError` if an error occurs
    /// during the orchestration process.
    pub async fn run(self) -> Result<(), OrchestratorError> {
        let (tx, mut rx) = mpsc::channel(1000); 
        
        let consumer_tx = tx.clone();
        let consumer = self.actions_consumer;
        let processor = self.actions_processor;
        let loader = self.actions_loader;

        // Wait until the tables are created
        loop {
            if loader.actions_repository.check_tables_created().await? {
                break;
            }
            println!("Waiting for tables to be created...");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }

        // Get the cursor from the database
        let cursor = loader.cursor_repository.get_cursor("actions_indexer").await.map_err(OrchestratorError::from)?;
        
        tokio::spawn(async move {
            if let Err(e) = consumer.run(consumer_tx, cursor).await {
                eprintln!("Consumer error: {:?}", e);
            }
        });
        
        while let Some(message) = rx.recv().await {
            match message {
                StreamMessage::BlockData(block_data) => {
                    let actions = block_data.actions;
                    let cursor = block_data.cursor;
                    let block_number = block_data.block_number;

                    if actions.len() > 0 {
                        let now = chrono::Utc::now();
                        println!("{} - Processing {} actions", now.to_rfc3339(), actions.len());
                        
                        let actions = processor.process(&actions);
                        
                        let mut votes: Vec<Vote> = Vec::new();
                        for action in actions.clone() {
                            match action {
                                Action::Vote(vote) => votes.push(vote),
                            }
                        }
                        
                        let user_votes = get_latest_user_votes(&votes);
                        let votes_count = update_vote_counts(&user_votes, loader.actions_repository.as_ref()).await?;

                        let changeset = Changeset { 
                            actions: &actions,  
                            user_votes: &user_votes,
                            votes_count: &votes_count,
                        };

                        if let Err(e) = loader.persist_changeset(&changeset).await {
                            eprintln!("Failed to persist changeset: {:?}", e);
                        } else {
                            save_cursor(&cursor, &block_number, loader.cursor_repository.as_ref()).await?;
                        }
                    } else {
                        if !cursor.is_empty() {
                            save_cursor(&cursor, &block_number, loader.cursor_repository.as_ref()).await?;
                        }
                    }

                }
                StreamMessage::UndoSignal(undo_signal) => {
                    println!("UndoSignal: {:?}", undo_signal);
                }
                StreamMessage::Error(error) => {
                    println!("Error: {:?}", error);
                }
                StreamMessage::StreamEnd => {
                    println!("StreamEnd");
                }
            }   
        }
        Ok(())
    }
}

#[derive(Debug)]
struct VotesDelta {
    upvotes: i32,
    downvotes: i32,
}

/// This method returns the latest vote for each user/entity/space combination
/// 
/// It assumes that the votes are sorted by block_timestamp so it simply returns the last occurrence
/// of each user/entity/space combination.
///
/// # Arguments
///
/// * `votes` - A slice of `Vote`s to process
///
/// # Returns
///
/// A vector of `UserVote`s with the latest vote for each user/entity/space combination.
///
fn get_latest_user_votes(votes: &[Vote]) -> Vec<UserVote> {
    let mut latest_votes: HashMap<VoteCriteria, &Vote> = HashMap::new();
    
    for vote in votes {
        let vote_criteria = (vote.raw.sender, vote.raw.object_id, vote.raw.space_pov, vote.raw.object_type);
        latest_votes.insert(vote_criteria, vote);
    }

    let mut user_votes = Vec::with_capacity(latest_votes.len());
    
    for ((user_id, object_id, space_id, object_type), vote) in latest_votes {
        user_votes.push(UserVote {
            user_id,
            object_id,
            object_type,
            space_id,
            vote_type: vote.vote.clone(),
            voted_at: vote.raw.block_timestamp,
        });
    }
    
    user_votes
}

/// This method updates the vote counts for each entity/space combination
///
/// It uses the user votes to calculate the vote changes and then updates the vote counts
/// for each entity/space combination.
///
/// # Arguments
///
/// * `user_votes` - A slice of `UserVote`s to process
/// * `actions_repository` - A reference to the `ActionsRepository` to use
///
/// # Returns
///
/// A vector of `VotesCount`s with the updated vote counts for each entity/space combination.
///
async fn update_vote_counts(user_votes: &[UserVote], actions_repository: &dyn ActionsRepository) -> Result<Vec<VotesCount>, OrchestratorError> {
    if user_votes.is_empty() {
        return Ok(Vec::new());
    }

    let vote_criteria: Vec<VoteCriteria> = user_votes.iter()
        .map(|vote| (vote.user_id, vote.object_id, vote.space_id, vote.object_type))
        .collect();
        
    let vote_count_criteria: Vec<VoteCountCriteria> = user_votes.iter()
        .map(|vote| (vote.object_id, vote.space_id, vote.object_type))
        .collect();

    let (stored_user_votes, stored_vote_counts) = tokio::try_join!(
        actions_repository.get_user_votes(&vote_criteria),
        actions_repository.get_vote_counts(&vote_count_criteria)
    )?;

    let stored_user_votes_map: HashMap<VoteCriteria, UserVote> = stored_user_votes
        .into_iter()
        .map(|vote| ((vote.user_id, vote.object_id, vote.space_id, vote.object_type), vote))
        .collect();

    let mut vote_counts_map: HashMap<VoteCountCriteria, VotesCount> = stored_vote_counts
        .into_iter()
        .map(|count| ((count.object_id, count.space_id, count.object_type), count))
        .collect();

    for new_vote in user_votes {
        let vote_criteria = (new_vote.user_id, new_vote.object_id, new_vote.space_id, new_vote.object_type);
        let count_criteria = (new_vote.object_id, new_vote.space_id, new_vote.object_type);
        
        let stored_user_vote = stored_user_votes_map.get(&vote_criteria);
        let vote_delta = compute_vote_delta(&stored_user_vote, new_vote);
        
        let vote_count = vote_counts_map.entry(count_criteria).or_insert_with(|| VotesCount {
            object_id: new_vote.object_id,
            object_type: new_vote.object_type,
            space_id: new_vote.space_id,
            upvotes: 0,
            downvotes: 0,
        });
        
        vote_count.upvotes += vote_delta.upvotes as i64;
        vote_count.downvotes += vote_delta.downvotes as i64;
    }

    Ok(vote_counts_map.into_values().collect())
}

fn compute_vote_delta(saved_vote: &Option<&UserVote>, new_vote: &UserVote) -> VotesDelta {
    let saved_vote_value = saved_vote.map(|vote| vote.vote_type.clone());
    let new_vote_value = new_vote.vote_type.clone();

    let (upvotes, downvotes) = match (saved_vote_value, new_vote_value) {
        (Some(VoteValue::Up), VoteValue::Down)          => (-1, 1),
        (Some(VoteValue::Up), VoteValue::Remove)        => (-1, 0),
        (Some(VoteValue::Down), VoteValue::Up)          => (1, -1),
        (Some(VoteValue::Down), VoteValue::Remove)      => (0, -1),
        (Some(VoteValue::Remove), VoteValue::Up)        => (1, 0),
        (Some(VoteValue::Remove), VoteValue::Down)      => (0, 1),
        (None, VoteValue::Up)                          => (1, 0),
        (None, VoteValue::Down)                        => (0, 1),
        (_, _) => (0, 0)
    };

    VotesDelta { upvotes, downvotes }
}

async fn save_cursor(cursor: &str, block_number: &i64, cursor_repository: &dyn CursorRepository) -> Result<(), OrchestratorError> {
    if let Err(e) = cursor_repository.save_cursor("actions_indexer", cursor, block_number).await {
        eprintln!("Failed to save cursor to database: {:?}", e);
        return Err(OrchestratorError::from(e));
    }
    Ok(())
}

#[cfg(test)]    
mod tests {
    use alloy::primitives::Address;
    use uuid::uuid;
    use alloy::hex::FromHex;
    use super::*;
    use actions_indexer_shared::types::{ObjectType, ActionType};

    pub fn dead_address() -> Address {
        Address::from_hex("0x000000000000000000000000000000000000dEaD").unwrap()
    }

    #[tokio::test]
    async fn test_calculate_votes_changes_upvote_downvote() {
        let prev_vote = UserVote {
            user_id: dead_address(),
            object_id: uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5"),
            object_type: ObjectType::Entity,
            space_id: uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"),
            vote_type: VoteValue::Up,
            voted_at: 1713859200,
        };
        
        let new_vote = UserVote {
            user_id: dead_address(),
            object_id: uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5"),
            object_type: ObjectType::Entity,
            space_id: uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"),
            vote_type: VoteValue::Down,
            voted_at: 1713859200,
        };
        
        let votes_changes = compute_vote_delta(&Some(&prev_vote), &new_vote);
        assert_eq!(votes_changes.upvotes, -1);
        assert_eq!(votes_changes.downvotes, 1);
    }

    #[tokio::test]
    async fn test_calculate_votes_changes_upvote_remove() {
        let prev_vote = UserVote {
            user_id: dead_address(),
            object_id: uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5"),
            object_type: ObjectType::Entity,
            space_id: uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"),
            vote_type: VoteValue::Up,
            voted_at: 1713859200,
        };
        
        let new_vote = UserVote {
            user_id: dead_address(),
            object_id: uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5"),
            object_type: ObjectType::Entity,
            space_id: uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"),
            vote_type: VoteValue::Remove,
            voted_at: 1713859200,
        };
        
        let votes_changes = compute_vote_delta(&Some(&prev_vote), &new_vote);
        assert_eq!(votes_changes.upvotes, -1);
        assert_eq!(votes_changes.downvotes, 0);
    }

    #[tokio::test]
    async fn test_calculate_votes_changes_downvote_upvote() {
        let prev_vote = UserVote {
            user_id: dead_address(),
            object_id: uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5"),
            object_type: ObjectType::Entity,
            space_id: uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"),
            vote_type: VoteValue::Down,
            voted_at: 1713859200,
        };
        
        let new_vote = UserVote {
            user_id: dead_address(),
            object_id: uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5"),
            object_type: ObjectType::Entity,
            space_id: uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"),
            vote_type: VoteValue::Up,
            voted_at: 1713859200,
        };
        
        let votes_changes = compute_vote_delta(&Some(&prev_vote), &new_vote);
        assert_eq!(votes_changes.upvotes, 1);
        assert_eq!(votes_changes.downvotes, -1);
    }

    #[tokio::test]
    async fn test_calculate_votes_changes_downvote_remove() {
        let prev_vote = UserVote {
            user_id: dead_address(),
            object_id: uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5"),
            object_type: ObjectType::Entity,
            space_id: uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"),
            vote_type: VoteValue::Down,
            voted_at: 1713859200,
        };

        let new_vote = UserVote {
            user_id: dead_address(),
            object_id: uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5"),
            object_type: ObjectType::Entity,
            space_id: uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"),
            vote_type: VoteValue::Remove,
            voted_at: 1713859200,
        };

        let votes_changes = compute_vote_delta(&Some(&prev_vote), &new_vote);
        assert_eq!(votes_changes.upvotes, 0);
        assert_eq!(votes_changes.downvotes, -1);
    }

    // ============================================================================
    // get_latest_user_votes Tests
    // ============================================================================

    #[tokio::test]
    async fn test_get_latest_user_votes_single_vote() {
        use actions_indexer_shared::types::{ActionRaw, Vote};
        use alloy::primitives::TxHash;
        
        let raw_action = ActionRaw {
            action_type: ActionType::Vote,
            action_version: 1,
            sender: dead_address(),
            object_id: uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5"),
            group_id: None,
            space_pov: uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"),
            metadata: None,
            block_number: 1,
            block_timestamp: 1713859200,
            tx_hash: TxHash::from_hex("0x5427daee8d03277f8a30ea881692c04861e692ce5f305b7a689b76248cae63c4").unwrap(),
            object_type: ObjectType::Entity,
        };

        let vote = Vote {
            raw: raw_action.clone(),
            vote: VoteValue::Up,
        };

        let votes = vec![vote];
        let user_votes = get_latest_user_votes(&votes);

        assert_eq!(user_votes.len(), 1);
        assert_eq!(user_votes[0].user_id, dead_address());
        assert_eq!(user_votes[0].object_id, uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5"));
        assert_eq!(user_votes[0].space_id, uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"));
        assert_eq!(user_votes[0].vote_type, VoteValue::Up);
        assert_eq!(user_votes[0].voted_at, 1713859200);
    }

    #[tokio::test]
    async fn test_get_latest_user_votes_empty_input() {
        let votes: Vec<Vote> = Vec::new();
        let user_votes = get_latest_user_votes(&votes);
        assert!(user_votes.is_empty());
    }

    #[tokio::test]
    async fn test_get_latest_user_votes_multiple_votes_same_user_same_entity() {
        use actions_indexer_shared::types::{ActionRaw, Vote};
        use alloy::primitives::TxHash;
        
        let base_raw = ActionRaw {
            action_type: ActionType::Vote,
            action_version: 1,
            sender: dead_address(),
            object_id: uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5"),
            group_id: None,
            space_pov: uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"),
            metadata: None,
            block_number: 1,
            block_timestamp: 1713859200,
            tx_hash: TxHash::from_hex("0x5427daee8d03277f8a30ea881692c04861e692ce5f305b7a689b76248cae63c4").unwrap(),
            object_type: ObjectType::Entity,
        };

        // First vote (older)
        let vote1 = Vote {
            raw: ActionRaw {
                block_timestamp: 1713859200,
                ..base_raw.clone()
            },
            vote: VoteValue::Up,
        };

        // Second vote (newer) - should be the one returned
        let vote2 = Vote {
            raw: ActionRaw {
                block_timestamp: 1713859300,
                tx_hash: TxHash::from_hex("0x6538dbff9d04388e9ac36264cf493b8c96e05421e59ead18b6e6547bc3d72fc5").unwrap(),
                ..base_raw.clone()
            },
            vote: VoteValue::Down,
        };

        let votes = vec![vote1, vote2.clone()];
        let user_votes = get_latest_user_votes(&votes);

        // Should only return one vote (the latest one)
        assert_eq!(user_votes.len(), 1);
        assert_eq!(user_votes[0].user_id, dead_address());
        assert_eq!(user_votes[0].object_id, uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5"));
        assert_eq!(user_votes[0].space_id, uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"));
        assert_eq!(user_votes[0].vote_type, VoteValue::Down);
        assert_eq!(user_votes[0].voted_at, 1713859300);
    }

    #[tokio::test]
    async fn test_get_latest_user_votes_multiple_users_same_entity() {
        use actions_indexer_shared::types::{ActionRaw, Vote};
        use alloy::primitives::TxHash;
        
        let user1 = dead_address();
        let user2 = Address::from_hex("0x1234567890123456789012345678901234567890").unwrap();
        let entity_id = uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5");
        
        let vote1 = Vote {
            raw: ActionRaw {
                action_type: ActionType::Vote,
                action_version: 1,
                sender: user1,
                object_id: entity_id,
                group_id: None,
                space_pov: uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"),
                metadata: None,
                block_number: 1,
                block_timestamp: 1713859200,
                tx_hash: TxHash::from_hex("0x5427daee8d03277f8a30ea881692c04861e692ce5f305b7a689b76248cae63c4").unwrap(),
                object_type: ObjectType::Entity,
            },
            vote: VoteValue::Up,
        };

        let vote2 = Vote {
            raw: ActionRaw {
                action_type: ActionType::Vote,
                action_version: 1,
                sender: user2,
                object_id: entity_id,
                group_id: None,
                space_pov: uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"),
                metadata: None,
                block_number: 1,
                block_timestamp: 1713859300,
                tx_hash: TxHash::from_hex("0x6538dbff9d04388e9ac36264cf493b8c96e05421e59ead18b6e6547bc3d72fc5").unwrap(),
                object_type: ObjectType::Entity,
            },
            vote: VoteValue::Down,
        };

        let votes = vec![vote1, vote2];
        let user_votes = get_latest_user_votes(&votes);

        // Should return both votes since they are from different users
        assert_eq!(user_votes.len(), 2);
        
        // Find the vote from each user
        let user1_vote = user_votes.iter().find(|v| v.user_id == user1).unwrap();
        let user2_vote = user_votes.iter().find(|v| v.user_id == user2).unwrap();
        
        assert_eq!(user1_vote.vote_type, VoteValue::Up);
        assert_eq!(user1_vote.voted_at, 1713859200);
        
        assert_eq!(user2_vote.vote_type, VoteValue::Down);
        assert_eq!(user2_vote.voted_at, 1713859300);
    }

    #[tokio::test]
    async fn test_get_latest_user_votes_same_user_different_entities() {
        use actions_indexer_shared::types::{ActionRaw, Vote};
        use alloy::primitives::TxHash;
        
        let user = dead_address();
        let entity1 = uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5");
        let entity2 = uuid!("b8f00127-b3f5-55fc-92db-b5f6c72e3cf6");
        
        let vote1 = Vote {
            raw: ActionRaw {
                action_type: ActionType::Vote,
                action_version: 1,
                sender: user,
                object_id: entity1,
                group_id: None,
                space_pov: uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"),
                metadata: None,
                block_number: 1,
                block_timestamp: 1713859200,
                tx_hash: TxHash::from_hex("0x5427daee8d03277f8a30ea881692c04861e692ce5f305b7a689b76248cae63c4").unwrap(),
                object_type: ObjectType::Entity,
            },
            vote: VoteValue::Up,
        };

        let vote2 = Vote {
            raw: ActionRaw {
                action_type: ActionType::Vote,
                action_version: 1,
                sender: user,
                object_id: entity2,
                group_id: None,
                space_pov: uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"),
                metadata: None,
                block_number: 1,
                block_timestamp: 1713859300,
                tx_hash: TxHash::from_hex("0x6538dbff9d04388e9ac36264cf493b8c96e05421e59ead18b6e6547bc3d72fc5").unwrap(),
                object_type: ObjectType::Entity,
            },
            vote: VoteValue::Remove,
        };

        let votes = vec![vote1, vote2];
        let user_votes = get_latest_user_votes(&votes);

        // Should return both votes since they are for different entities
        assert_eq!(user_votes.len(), 2);
        
        // Find the vote for each entity
        let entity1_vote = user_votes.iter().find(|v| v.object_id == entity1).unwrap();
        let entity2_vote = user_votes.iter().find(|v| v.object_id == entity2).unwrap();
        
        assert_eq!(entity1_vote.vote_type, VoteValue::Up);
        assert_eq!(entity1_vote.voted_at, 1713859200);
        
        assert_eq!(entity2_vote.vote_type, VoteValue::Remove);
        assert_eq!(entity2_vote.voted_at, 1713859300);
    }

    #[tokio::test]
    async fn test_get_latest_user_votes_different_vote_types() {
        use actions_indexer_shared::types::{ActionRaw, Vote};
        use alloy::primitives::TxHash;
        
        let user1 = dead_address();
        let user2 = Address::from_hex("0x1234567890123456789012345678901234567890").unwrap();
        let user3 = Address::from_hex("0x9876543210987654321098765432109876543210").unwrap();
        let entity_id = uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5");
        
        let upvote = Vote {
            raw: ActionRaw {
                action_type: ActionType::Vote,
                action_version: 1,
                sender: user1,
                object_id: entity_id,
                group_id: None,
                space_pov: uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"),
                metadata: None,
                block_number: 1,
                block_timestamp: 1713859200,
                tx_hash: TxHash::from_hex("0x5427daee8d03277f8a30ea881692c04861e692ce5f305b7a689b76248cae63c4").unwrap(),
                object_type: ObjectType::Entity,
            },
            vote: VoteValue::Up,
        };

        let downvote = Vote {
            raw: ActionRaw {
                action_type: ActionType::Vote,
                action_version: 1,
                sender: user2,
                object_id: entity_id,
                group_id: None,
                space_pov: uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"),
                metadata: None,
                block_number: 1,
                block_timestamp: 1713859300,
                tx_hash: TxHash::from_hex("0x6538dbff9d04388e9ac36264cf493b8c96e05421e59ead18b6e6547bc3d72fc5").unwrap(),
                object_type: ObjectType::Entity,
            },
            vote: VoteValue::Down,
        };

        let remove_vote = Vote {
            raw: ActionRaw {
                action_type: ActionType::Vote,
                action_version: 1,
                sender: user3,
                object_id: entity_id,
                group_id: None,
                space_pov: uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"),
                metadata: None,
                block_number: 1,
                block_timestamp: 1713859400,
                tx_hash: TxHash::from_hex("0x7649ec009e05499f9bd47274ef4e73a6f7b24126f79ead19c6e6648cd4e83af6").unwrap(),
                object_type: ObjectType::Entity,
            },
            vote: VoteValue::Remove,
        };

        let votes = vec![upvote, downvote, remove_vote];
        let user_votes = get_latest_user_votes(&votes);

        // Should return all three votes since they are from different users
        assert_eq!(user_votes.len(), 3);
        
        // Verify each vote type is correctly preserved
        let user1_vote = user_votes.iter().find(|v| v.user_id == user1).unwrap();
        let user2_vote = user_votes.iter().find(|v| v.user_id == user2).unwrap();
        let user3_vote = user_votes.iter().find(|v| v.user_id == user3).unwrap();
        
        assert_eq!(user1_vote.vote_type, VoteValue::Up);
        assert_eq!(user2_vote.vote_type, VoteValue::Down);
        assert_eq!(user3_vote.vote_type, VoteValue::Remove);
    }

    #[tokio::test]
    async fn test_get_latest_user_votes_same_user_different_spaces() {
        use actions_indexer_shared::types::{ActionRaw, Vote};
        use alloy::primitives::TxHash;
        
        let user = dead_address();
        let entity_id = uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5");
        let space1 = uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b");
        let space2 = uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318a");
        
        let vote1 = Vote {
            raw: ActionRaw {
                action_type: ActionType::Vote,
                action_version: 1,
                sender: user,
                object_id: entity_id,
                group_id: None,
                space_pov: space1,
                metadata: None,
                block_number: 1,
                block_timestamp: 1713859200,
                tx_hash: TxHash::from_hex("0x5427daee8d03277f8a30ea881692c04861e692ce5f305b7a689b76248cae63c4").unwrap(),
                object_type: ObjectType::Entity,
            },
            vote: VoteValue::Up,
        };

        let vote2 = Vote {
            raw: ActionRaw {
                action_type: ActionType::Vote,
                action_version: 1,
                sender: user,
                object_id: entity_id,
                group_id: None,
                space_pov: space2,
                metadata: None,
                block_number: 1,
                block_timestamp: 1713859300,
                tx_hash: TxHash::from_hex("0x6538dbff9d04388e9ac36264cf493b8c96e05421e59ead18b6e6547bc3d72fc5").unwrap(),
                object_type: ObjectType::Entity,
            },
            vote: VoteValue::Down,
        };

        let votes = vec![vote1, vote2];
        let user_votes = get_latest_user_votes(&votes);

        // Should return both votes since they are for different spaces
        assert_eq!(user_votes.len(), 2);
        
        // Find the vote for each space
        let space1_vote = user_votes.iter().find(|v| v.space_id == space1).unwrap();
        let space2_vote = user_votes.iter().find(|v| v.space_id == space2).unwrap();
        
        assert_eq!(space1_vote.vote_type, VoteValue::Up);
        assert_eq!(space1_vote.voted_at, 1713859200);
        
        assert_eq!(space2_vote.vote_type, VoteValue::Down);
        assert_eq!(space2_vote.voted_at, 1713859300);
    }

    #[tokio::test]
    async fn test_get_latest_user_votes_same_user_different_object_type() {
        use actions_indexer_shared::types::{ActionRaw, Vote};
        use alloy::primitives::TxHash;
        
        let user = dead_address();
        let object = uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5");

        let vote1 = Vote {
            raw: ActionRaw {
                action_type: ActionType::Vote,
                action_version: 1,
                sender: user,
                object_id: object,
                group_id: None,
                space_pov: uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"),
                metadata: None,
                block_number: 1,
                block_timestamp: 1713859200,
                tx_hash: TxHash::from_hex("0x5427daee8d03277f8a30ea881692c04861e692ce5f305b7a689b76248cae63c4").unwrap(),
                object_type: ObjectType::Entity,
            },
            vote: VoteValue::Up,
        };

        let vote2 = Vote {
            raw: ActionRaw {
                action_type: ActionType::Vote,
                action_version: 1,
                sender: user,
                object_id: object,
                group_id: None,
                space_pov: uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b"),
                metadata: None,
                block_number: 1,
                block_timestamp: 1713859200,
                tx_hash: TxHash::from_hex("0x5427daee8d03277f8a30ea881692c04861e692ce5f305b7a689b76248cae63c4").unwrap(),
                object_type: ObjectType::Relation, // Different object type
            },
            vote: VoteValue::Up,
        };

        let votes = vec![vote1, vote2];
        let user_votes = get_latest_user_votes(&votes);

        // Should return both votes since they are for different object types
        assert_eq!(user_votes.len(), 2);
        
        // All votes have the same object_id and user_id
        assert_eq!(user_votes.iter().all(|v| v.object_id == object), true);
        assert_eq!(user_votes.iter().all(|v| v.user_id == user), true);

        // All votes have different object types
        assert_eq!(user_votes.iter().any(|v| v.object_type == ObjectType::Entity), true);
        assert_eq!(user_votes.iter().any(|v| v.object_type == ObjectType::Relation), true);
        
    }

    // ============================================================================
    // update_vote_counts Tests
    // ============================================================================

    struct MockActionsRepository {
        stored_user_votes: Vec<UserVote>,
        stored_vote_counts: Vec<VotesCount>,
    }

    #[async_trait::async_trait]
    impl ActionsRepository for MockActionsRepository {
        async fn insert_actions(&self, _actions: &[Action]) -> Result<(), actions_indexer_repository::errors::ActionsRepositoryError> {
            unimplemented!()
        }

        async fn update_user_votes(&self, _user_votes: &[UserVote]) -> Result<(), actions_indexer_repository::errors::ActionsRepositoryError> {
            unimplemented!()
        }

        async fn update_votes_counts(&self, _votes_counts: &[VotesCount]) -> Result<(), actions_indexer_repository::errors::ActionsRepositoryError> {
            unimplemented!()
        }

        async fn persist_changeset(&self, _changeset: &Changeset<'_>) -> Result<(), actions_indexer_repository::errors::ActionsRepositoryError> {
            unimplemented!()
        }

        async fn get_user_votes(&self, _vote_criteria: &[VoteCriteria]) -> Result<Vec<UserVote>, actions_indexer_repository::errors::ActionsRepositoryError> {
            Ok(self.stored_user_votes.clone())
        }

        async fn get_vote_counts(&self, _vote_criteria: &[VoteCountCriteria]) -> Result<Vec<VotesCount>, actions_indexer_repository::errors::ActionsRepositoryError> {
            Ok(self.stored_vote_counts.clone())
        }

        async fn check_tables_created(&self) -> Result<bool, actions_indexer_repository::errors::ActionsRepositoryError> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn test_update_vote_counts_empty_input() {
        let mock_repo = MockActionsRepository {
            stored_user_votes: vec![],
            stored_vote_counts: vec![],
        };

        let user_votes: Vec<UserVote> = vec![];
        let result = update_vote_counts(&user_votes, &mock_repo).await;

        assert!(result.is_ok());
        let vote_counts = result.unwrap();
        assert_eq!(vote_counts.len(), 0);
    }

    #[tokio::test]
    async fn test_update_vote_counts_new_upvote_no_existing_data() {
        let user = dead_address();
        let object_id = uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5");
        let space_id = uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b");

        let mock_repo = MockActionsRepository {
            stored_user_votes: vec![],
            stored_vote_counts: vec![],
        };

        let user_votes = vec![UserVote {
            user_id: user,
            object_id,
            object_type: ObjectType::Entity,
            space_id,
            vote_type: VoteValue::Up,
            voted_at: 1713859200,
        }];

        let result = update_vote_counts(&user_votes, &mock_repo).await;

        assert!(result.is_ok());
        let vote_counts = result.unwrap();
        assert_eq!(vote_counts.len(), 1);
        assert_eq!(vote_counts[0].object_id, object_id);
        assert_eq!(vote_counts[0].space_id, space_id);
        assert_eq!(vote_counts[0].object_type, ObjectType::Entity);
        assert_eq!(vote_counts[0].upvotes, 1);
        assert_eq!(vote_counts[0].downvotes, 0);
    }

    #[tokio::test]
    async fn test_update_vote_counts_change_upvote_to_downvote() {
        let user = dead_address();
        let object_id = uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5");
        let space_id = uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b");

        let mock_repo = MockActionsRepository {
            stored_user_votes: vec![UserVote {
                user_id: user,
                object_id,
                object_type: ObjectType::Entity,
                space_id,
                vote_type: VoteValue::Up,
                voted_at: 1713859100,
            }],
            stored_vote_counts: vec![VotesCount {
                object_id,
                object_type: ObjectType::Entity,
                space_id,
                upvotes: 5,
                downvotes: 2,
            }],
        };

        let user_votes = vec![UserVote {
            user_id: user,
            object_id,
            object_type: ObjectType::Entity,
            space_id,
            vote_type: VoteValue::Down,
            voted_at: 1713859200,
        }];

        let result = update_vote_counts(&user_votes, &mock_repo).await;

        assert!(result.is_ok());
        let vote_counts = result.unwrap();
        assert_eq!(vote_counts.len(), 1);
        assert_eq!(vote_counts[0].upvotes, 4); // 5 - 1
        assert_eq!(vote_counts[0].downvotes, 3); // 2 + 1
    }

    #[tokio::test]
    async fn test_update_vote_counts_change_downvote_to_upvote() {
        let user = dead_address();
        let object_id = uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5");
        let space_id = uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b");

        let mock_repo = MockActionsRepository {
            stored_user_votes: vec![UserVote {
                user_id: user,
                object_id,
                object_type: ObjectType::Entity,
                space_id,
                vote_type: VoteValue::Down,
                voted_at: 1713859100,
            }],
            stored_vote_counts: vec![VotesCount {
                object_id,
                object_type: ObjectType::Entity,
                space_id,
                upvotes: 3,
                downvotes: 7,
            }],
        };

        let user_votes = vec![UserVote {
            user_id: user,
            object_id,
            object_type: ObjectType::Entity,
            space_id,
            vote_type: VoteValue::Up,
            voted_at: 1713859200,
        }];

        let result = update_vote_counts(&user_votes, &mock_repo).await;

        assert!(result.is_ok());
        let vote_counts = result.unwrap();
        assert_eq!(vote_counts.len(), 1);
        assert_eq!(vote_counts[0].upvotes, 4); // 3 + 1
        assert_eq!(vote_counts[0].downvotes, 6); // 7 - 1
    }

    #[tokio::test]
    async fn test_update_vote_counts_remove_upvote() {
        let user = dead_address();
        let object_id = uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5");
        let space_id = uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b");

        let mock_repo = MockActionsRepository {
            stored_user_votes: vec![UserVote {
                user_id: user,
                object_id,
                object_type: ObjectType::Entity,
                space_id,
                vote_type: VoteValue::Up,
                voted_at: 1713859100,
            }],
            stored_vote_counts: vec![VotesCount {
                object_id,
                object_type: ObjectType::Entity,
                space_id,
                upvotes: 10,
                downvotes: 5,
            }],
        };

        let user_votes = vec![UserVote {
            user_id: user,
            object_id,
            object_type: ObjectType::Entity,
            space_id,
            vote_type: VoteValue::Remove,
            voted_at: 1713859200,
        }];

        let result = update_vote_counts(&user_votes, &mock_repo).await;

        assert!(result.is_ok());
        let vote_counts = result.unwrap();
        assert_eq!(vote_counts.len(), 1);
        assert_eq!(vote_counts[0].upvotes, 9); // 10 - 1
        assert_eq!(vote_counts[0].downvotes, 5); // unchanged
    }

    #[tokio::test]
    async fn test_update_vote_counts_multiple_users_same_object() {
        let user1 = dead_address();
        let user2 = Address::from_hex("0x1234567890123456789012345678901234567890").unwrap();
        let object_id = uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5");
        let space_id = uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b");

        let mock_repo = MockActionsRepository {
            stored_user_votes: vec![],
            stored_vote_counts: vec![],
        };

        let user_votes = vec![
            UserVote {
                user_id: user1,
                object_id,
                object_type: ObjectType::Entity,
                space_id,
                vote_type: VoteValue::Up,
                voted_at: 1713859200,
            },
            UserVote {
                user_id: user2,
                object_id,
                object_type: ObjectType::Entity,
                space_id,
                vote_type: VoteValue::Down,
                voted_at: 1713859200,
            },
        ];

        let result = update_vote_counts(&user_votes, &mock_repo).await;

        assert!(result.is_ok());
        let vote_counts = result.unwrap();
        assert_eq!(vote_counts.len(), 1);
        assert_eq!(vote_counts[0].object_id, object_id);
        assert_eq!(vote_counts[0].upvotes, 1);
        assert_eq!(vote_counts[0].downvotes, 1);
    }

    #[tokio::test]
    async fn test_update_vote_counts_multiple_objects() {
        let user = dead_address();
        let object1 = uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5");
        let object2 = uuid!("b8f00127-b3f5-55fc-92db-b5f6c72e3cf6");
        let space_id = uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b");

        let mock_repo = MockActionsRepository {
            stored_user_votes: vec![],
            stored_vote_counts: vec![],
        };

        let user_votes = vec![
            UserVote {
                user_id: user,
                object_id: object1,
                object_type: ObjectType::Entity,
                space_id,
                vote_type: VoteValue::Up,
                voted_at: 1713859200,
            },
            UserVote {
                user_id: user,
                object_id: object2,
                object_type: ObjectType::Entity,
                space_id,
                vote_type: VoteValue::Up,
                voted_at: 1713859200,
            },
        ];

        let result = update_vote_counts(&user_votes, &mock_repo).await;

        assert!(result.is_ok());
        let vote_counts = result.unwrap();
        assert_eq!(vote_counts.len(), 2);
        
        let object1_count = vote_counts.iter().find(|v| v.object_id == object1).unwrap();
        let object2_count = vote_counts.iter().find(|v| v.object_id == object2).unwrap();
        
        assert_eq!(object1_count.upvotes, 1);
        assert_eq!(object1_count.downvotes, 0);
        assert_eq!(object2_count.upvotes, 1);
        assert_eq!(object2_count.downvotes, 0);
    }

    #[tokio::test]
    async fn test_update_vote_counts_same_vote_no_change() {
        let user = dead_address();
        let object_id = uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5");
        let space_id = uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b");

        let mock_repo = MockActionsRepository {
            stored_user_votes: vec![UserVote {
                user_id: user,
                object_id,
                object_type: ObjectType::Entity,
                space_id,
                vote_type: VoteValue::Up,
                voted_at: 1713859100,
            }],
            stored_vote_counts: vec![VotesCount {
                object_id,
                object_type: ObjectType::Entity,
                space_id,
                upvotes: 5,
                downvotes: 2,
            }],
        };

        let user_votes = vec![UserVote {
            user_id: user,
            object_id,
            object_type: ObjectType::Entity,
            space_id,
            vote_type: VoteValue::Up, // Same vote type
            voted_at: 1713859200,
        }];

        let result = update_vote_counts(&user_votes, &mock_repo).await;

        assert!(result.is_ok());
        let vote_counts = result.unwrap();
        assert_eq!(vote_counts.len(), 1);
        assert_eq!(vote_counts[0].upvotes, 5); // No change
        assert_eq!(vote_counts[0].downvotes, 2); // No change
    }

    #[tokio::test]
    async fn test_update_vote_counts_different_object_types() {
        let user = dead_address();
        let object_id = uuid!("a7ef0016-a2f4-44fb-82ca-a4f5c61d2cf5");
        let space_id = uuid!("e50fe85c-108a-4d4a-97b9-376a1e5d318b");

        let mock_repo = MockActionsRepository {
            stored_user_votes: vec![],
            stored_vote_counts: vec![],
        };

        let user_votes = vec![
            UserVote {
                user_id: user,
                object_id,
                object_type: ObjectType::Entity,
                space_id,
                vote_type: VoteValue::Up,
                voted_at: 1713859200,
            },
            UserVote {
                user_id: user,
                object_id,
                object_type: ObjectType::Relation,
                space_id,
                vote_type: VoteValue::Down,
                voted_at: 1713859200,
            },
        ];

        let result = update_vote_counts(&user_votes, &mock_repo).await;

        assert!(result.is_ok());
        let vote_counts = result.unwrap();
        assert_eq!(vote_counts.len(), 2);
        
        let entity_count = vote_counts.iter().find(|v| v.object_type == ObjectType::Entity).unwrap();
        let relation_count = vote_counts.iter().find(|v| v.object_type == ObjectType::Relation).unwrap();
        
        assert_eq!(entity_count.upvotes, 1);
        assert_eq!(entity_count.downvotes, 0);
        assert_eq!(relation_count.upvotes, 0);
        assert_eq!(relation_count.downvotes, 1);
    }
}