//! Handler for HermesVoteCast messages from the curation.votes topic.

use std::collections::HashMap;

use hermes_schema::pb::voting::{HermesVoteCast, VoteDirection, VoteKind};
use sdk::core::ids::GEO_SYSTEM_NAMESPACE;
use uuid::Uuid;

use crate::error::HandlerError;
use crate::models::voting::{
    ResponseKind, ScoreValueItem, UserVoteCriteria, UserVoteItem, VoteCountCriteria, VoteItem,
    VoteObjectType, VoteValue, VotesCountItem,
};

/// Object type discriminator values (big-endian 4-byte encoding)
const OBJECT_TYPE_ENTITY: [u8; 4] = [0x00, 0x00, 0x00, 0x00];
const OBJECT_TYPE_RELATION: [u8; 4] = [0x00, 0x00, 0x00, 0x01];

/// Parse object type from 4-byte discriminator
fn parse_object_type(bytes: &[u8]) -> Result<VoteObjectType, HandlerError> {
    if bytes.len() < 4 {
        return Err(HandlerError::InvalidObjectType(bytes.to_vec()));
    }

    let type_bytes: [u8; 4] = bytes[0..4].try_into().unwrap();

    match type_bytes {
        OBJECT_TYPE_ENTITY => Ok(VoteObjectType::Entity),
        OBJECT_TYPE_RELATION => Ok(VoteObjectType::Relation),
        _ => Err(HandlerError::InvalidObjectType(bytes.to_vec())),
    }
}

/// Convert HermesVoteCast to VoteItem
pub fn handle_vote_cast(vote: &HermesVoteCast) -> Result<VoteItem, HandlerError> {
    let meta = vote.meta.as_ref().ok_or(HandlerError::MissingPayload)?;

    let voter_id = Uuid::from_slice(&vote.voter_id)?;
    let object_id = Uuid::from_slice(&vote.object_id)?;
    let space_id = Uuid::from_slice(&vote.space_pov)?;
    let object_type = parse_object_type(&vote.object_type)?;

    let vote_value = match VoteDirection::try_from(vote.direction) {
        Ok(VoteDirection::Up) => VoteValue::Up,
        Ok(VoteDirection::Down) => VoteValue::Down,
        Ok(VoteDirection::None) => VoteValue::Remove,
        Err(_) => return Err(HandlerError::InvalidVoteDirection(vote.direction)),
    };

    // An unrecognised kind keeps its raw discriminant rather than falling back
    // to curation. Events produced before this field existed carry 0 and decode
    // as curation naturally; a *newer* producer emitting a kind this build
    // predates must not be folded into curation, or its rows would collide with
    // the user's real curation vote and overwrite it — the exact silent loss
    // vote_kind exists to prevent. Preserving the value keeps such rows in their
    // own key space, inert until this binary is upgraded, and does not halt the
    // consumer mid-batch.
    let kind = match VoteKind::try_from(vote.kind) {
        Ok(VoteKind::Curation) => ResponseKind::Curation,
        Ok(VoteKind::Stance) => ResponseKind::Stance,
        Ok(VoteKind::Veracity) => ResponseKind::Veracity,
        Err(_) => ResponseKind::Unknown(vote.kind as i16),
    };

    Ok(VoteItem {
        voter_id,
        object_id,
        object_type,
        space_id,
        vote: vote_value,
        kind,
        block_number: meta.block_number,
        block_timestamp: meta.created_at,
    })
}

/// Represents the change in tallies when a response is modified.
///
/// Always applies within a single kind — a response on one axis never moves
/// another axis's tallies.
#[derive(Debug, PartialEq, Eq)]
pub struct VotesDelta {
    pub positive: i32,
    pub negative: i32,
}

/// Deduplicate votes, keeping the latest per user/entity/space/object_type/KIND.
///
/// Assumes votes are processed in order (by block_timestamp), so the last
/// occurrence for each unique key is the most recent response.
///
/// The kind is part of the dedup key: an upvote and a Verify cast by the same
/// user on the same object in the same block are two distinct responses, and
/// collapsing them would drop one.
pub fn get_latest_user_votes(votes: &[VoteItem]) -> Vec<UserVoteItem> {
    let mut latest_votes: HashMap<UserVoteCriteria, &VoteItem> = HashMap::new();

    for vote in votes {
        let vote_criteria = (
            vote.voter_id,
            vote.object_id,
            vote.space_id,
            vote.object_type,
            vote.kind,
        );
        latest_votes.insert(vote_criteria, vote);
    }

    latest_votes
        .into_iter()
        .map(
            |((voter_id, object_id, space_id, object_type, kind), vote)| UserVoteItem {
                voter_id,
                object_id,
                object_type,
                space_id,
                vote_type: vote.vote.clone(),
                kind,
                voted_at: vote.block_timestamp,
            },
        )
        .collect()
}

/// Compute the delta in positive/negative tallies when a response changes.
///
/// Returns the change that should be applied to the aggregate counts.
pub fn compute_vote_delta(
    saved_vote: Option<&UserVoteItem>,
    new_vote: &UserVoteItem,
) -> VotesDelta {
    let saved_vote_value = saved_vote.map(|vote| vote.vote_type.clone());
    let new_vote_value = new_vote.vote_type.clone();

    let (positive, negative) = match (saved_vote_value, new_vote_value) {
        (Some(VoteValue::Up), VoteValue::Down) => (-1, 1),
        (Some(VoteValue::Up), VoteValue::Remove) => (-1, 0),
        (Some(VoteValue::Down), VoteValue::Up) => (1, -1),
        (Some(VoteValue::Down), VoteValue::Remove) => (0, -1),
        (Some(VoteValue::Remove), VoteValue::Up) => (1, 0),
        (Some(VoteValue::Remove), VoteValue::Down) => (0, 1),
        (None, VoteValue::Up) => (1, 0),
        (None, VoteValue::Down) => (0, 1),
        // No change for same vote type or Remove -> Remove
        (_, _) => (0, 0),
    };

    VotesDelta { positive, negative }
}

/// Calculate updated vote counts based on new votes and existing stored data.
///
/// This function takes:
/// - `user_votes`: The new votes to process
/// - `stored_user_votes`: Existing user votes from the database (keyed by criteria)
/// - `stored_vote_counts`: Existing vote counts from the database (keyed by criteria)
///
/// Returns the updated vote counts that should be upserted to the database.
pub fn calculate_vote_counts(
    user_votes: &[UserVoteItem],
    stored_user_votes: &HashMap<UserVoteCriteria, UserVoteItem>,
    stored_vote_counts: &HashMap<VoteCountCriteria, VotesCountItem>,
) -> Vec<VotesCountItem> {
    let mut vote_counts_map: HashMap<VoteCountCriteria, VotesCountItem> =
        stored_vote_counts.clone();

    for new_vote in user_votes {
        // Both keys carry the kind, so a delta computed for one axis can only
        // ever land on that axis's aggregate row.
        let vote_criteria = (
            new_vote.voter_id,
            new_vote.object_id,
            new_vote.space_id,
            new_vote.object_type,
            new_vote.kind,
        );
        let count_criteria = (
            new_vote.object_id,
            new_vote.space_id,
            new_vote.object_type,
            new_vote.kind,
        );

        let stored_user_vote = stored_user_votes.get(&vote_criteria);
        let vote_delta = compute_vote_delta(stored_user_vote, new_vote);

        let vote_count = vote_counts_map
            .entry(count_criteria)
            .or_insert_with(|| VotesCountItem {
                object_id: new_vote.object_id,
                object_type: new_vote.object_type,
                space_id: new_vote.space_id,
                kind: new_vote.kind,
                positive: 0,
                negative: 0,
            });

        vote_count.positive += vote_delta.positive as i64;
        vote_count.negative += vote_delta.negative as i64;
    }

    vote_counts_map.into_values().collect()
}

/// Build the `values`-table rows mirroring net scores for entities.
///
/// Skips relation votes: only entities (`object_type == Entity`) get a score row.
///
/// Also skips every non-curation kind, and the reason is narrower than it used to
/// claim. The previous wording here was "ranking stays curation-only (PRD §8
/// Q4)". Both halves were wrong: the PRD has no §8 and no numbered questions, and
/// since 0078 the ranking is NOT curation-only — stance feeds its participation
/// term. That citation is why claims carrying positions ranked below claims with
/// none for weeks; it read as a settled decision and nobody re-examined it.
///
/// The real and still-valid reason is a key collision, local to this function:
/// the row `id` is a UUIDv5 over `score:<entity>:<space>` with no kind in it, so
/// a claim's agrees and verifications would be written over each other's score
/// row and whichever kind was processed last would silently become the entity's
/// score. Adding a kind here means widening that id scheme, not relaxing a
/// filter. See `storage::ranking_recompute_entity_ids` for the ranking's own
/// kind policy, which is now deliberately different from this one.
/// The row `id` is a deterministic UUIDv5 over the name `score:<entity>:<space>`
/// under `GEO_SYSTEM_NAMESPACE` — the `score:` tag keeps these ids disjoint from
/// any other scheme that might hash `(entity_id, space_id)`.
pub fn build_score_values(counts: &[VotesCountItem]) -> Vec<ScoreValueItem> {
    let ns = Uuid::parse_str(GEO_SYSTEM_NAMESPACE)
        .expect("GEO_SYSTEM_NAMESPACE is a valid UUID constant");
    counts
        .iter()
        .filter(|c| c.object_type == VoteObjectType::Entity)
        .filter(|c| c.kind == ResponseKind::Curation)
        .map(|c| ScoreValueItem {
            id: derive_score_value_id(&ns, &c.object_id, &c.space_id),
            entity_id: c.object_id,
            space_id: c.space_id,
            integer: c.positive - c.negative,
        })
        .collect()
}

/// Derive the `values.id` for a score row as UUIDv5 of `score:<entity>:<space>`.
fn derive_score_value_id(namespace: &Uuid, entity_id: &Uuid, space_id: &Uuid) -> Uuid {
    let name = format!("score:{entity_id}:{space_id}");
    Uuid::new_v5(namespace, name.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;

    fn make_test_meta() -> BlockchainMetadata {
        BlockchainMetadata {
            block_number: 12345,
            created_at: 1700000000,
            sequence: 0,
            is_last: true,
            ..Default::default()
        }
    }

    fn make_test_uuid() -> Vec<u8> {
        vec![
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,
            0x0F, 0x10,
        ]
    }

    #[test]
    fn test_handle_vote_cast_upvote_entity() {
        let vote = HermesVoteCast {
            voter_id: make_test_uuid(),
            object_type: OBJECT_TYPE_ENTITY.to_vec(),
            object_id: make_test_uuid(),
            direction: VoteDirection::Up as i32,
            version: 1,
            group_id: make_test_uuid(),
            space_pov: make_test_uuid(),
            meta: Some(make_test_meta()),
            kind: VoteKind::Curation as i32,
        };

        let result = handle_vote_cast(&vote).unwrap();

        assert_eq!(result.object_type, VoteObjectType::Entity);
        assert_eq!(result.vote, VoteValue::Up);
        assert_eq!(result.block_number, 12345);
        assert_eq!(result.block_timestamp, 1700000000);
    }

    #[test]
    fn test_handle_vote_cast_downvote_relation() {
        let vote = HermesVoteCast {
            voter_id: make_test_uuid(),
            object_type: OBJECT_TYPE_RELATION.to_vec(),
            object_id: make_test_uuid(),
            direction: VoteDirection::Down as i32,
            version: 1,
            group_id: make_test_uuid(),
            space_pov: make_test_uuid(),
            meta: Some(make_test_meta()),
            kind: VoteKind::Curation as i32,
        };

        let result = handle_vote_cast(&vote).unwrap();

        assert_eq!(result.object_type, VoteObjectType::Relation);
        assert_eq!(result.vote, VoteValue::Down);
    }

    #[test]
    fn test_handle_vote_cast_unvote() {
        let vote = HermesVoteCast {
            voter_id: make_test_uuid(),
            object_type: OBJECT_TYPE_ENTITY.to_vec(),
            object_id: make_test_uuid(),
            direction: VoteDirection::None as i32,
            version: 1,
            group_id: make_test_uuid(),
            space_pov: make_test_uuid(),
            meta: Some(make_test_meta()),
            kind: VoteKind::Curation as i32,
        };

        let result = handle_vote_cast(&vote).unwrap();

        assert_eq!(result.vote, VoteValue::Remove);
    }

    #[test]
    fn test_handle_vote_cast_missing_meta() {
        let vote = HermesVoteCast {
            voter_id: make_test_uuid(),
            object_type: OBJECT_TYPE_ENTITY.to_vec(),
            object_id: make_test_uuid(),
            direction: VoteDirection::Up as i32,
            version: 1,
            group_id: make_test_uuid(),
            space_pov: make_test_uuid(),
            meta: None,
            kind: VoteKind::Curation as i32,
        };

        let result = handle_vote_cast(&vote);
        assert!(matches!(result, Err(HandlerError::MissingPayload)));
    }

    #[test]
    fn test_handle_vote_cast_invalid_object_type() {
        let vote = HermesVoteCast {
            voter_id: make_test_uuid(),
            object_type: vec![0xFF, 0xFF, 0xFF, 0xFF],
            object_id: make_test_uuid(),
            direction: VoteDirection::Up as i32,
            version: 1,
            group_id: make_test_uuid(),
            space_pov: make_test_uuid(),
            meta: Some(make_test_meta()),
            kind: VoteKind::Curation as i32,
        };

        let result = handle_vote_cast(&vote);
        assert!(matches!(result, Err(HandlerError::InvalidObjectType(_))));
    }

    #[test]
    fn test_parse_object_type_entity() {
        assert_eq!(
            parse_object_type(&OBJECT_TYPE_ENTITY).unwrap(),
            VoteObjectType::Entity
        );
    }

    #[test]
    fn test_parse_object_type_relation() {
        assert_eq!(
            parse_object_type(&OBJECT_TYPE_RELATION).unwrap(),
            VoteObjectType::Relation
        );
    }

    #[test]
    fn test_parse_object_type_too_short() {
        let result = parse_object_type(&[0x00, 0x00]);
        assert!(matches!(result, Err(HandlerError::InvalidObjectType(_))));
    }

    // ============================================================================
    // get_latest_user_votes Tests
    // ============================================================================

    fn make_vote_item(
        voter_id: Uuid,
        object_id: Uuid,
        space_id: Uuid,
        object_type: VoteObjectType,
        vote: VoteValue,
        block_timestamp: u64,
    ) -> VoteItem {
        make_kinded_vote_item(
            voter_id,
            object_id,
            space_id,
            object_type,
            vote,
            ResponseKind::Curation,
            block_timestamp,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn make_kinded_vote_item(
        voter_id: Uuid,
        object_id: Uuid,
        space_id: Uuid,
        object_type: VoteObjectType,
        vote: VoteValue,
        kind: ResponseKind,
        block_timestamp: u64,
    ) -> VoteItem {
        VoteItem {
            voter_id,
            object_id,
            object_type,
            space_id,
            vote,
            kind,
            block_number: 1,
            block_timestamp,
        }
    }

    #[test]
    fn test_get_latest_user_votes_single_vote() {
        let voter = Uuid::from_bytes([1u8; 16]);
        let object = Uuid::from_bytes([2u8; 16]);
        let space = Uuid::from_bytes([3u8; 16]);

        let vote = make_vote_item(
            voter,
            object,
            space,
            VoteObjectType::Entity,
            VoteValue::Up,
            1000,
        );
        let votes = vec![vote];

        let user_votes = get_latest_user_votes(&votes);

        assert_eq!(user_votes.len(), 1);
        assert_eq!(user_votes[0].voter_id, voter);
        assert_eq!(user_votes[0].object_id, object);
        assert_eq!(user_votes[0].space_id, space);
        assert_eq!(user_votes[0].vote_type, VoteValue::Up);
    }

    #[test]
    fn test_get_latest_user_votes_empty_input() {
        let votes: Vec<VoteItem> = Vec::new();
        let user_votes = get_latest_user_votes(&votes);
        assert!(user_votes.is_empty());
    }

    #[test]
    fn test_get_latest_user_votes_same_user_same_entity_keeps_latest() {
        let voter = Uuid::from_bytes([1u8; 16]);
        let object = Uuid::from_bytes([2u8; 16]);
        let space = Uuid::from_bytes([3u8; 16]);

        let vote1 = make_vote_item(
            voter,
            object,
            space,
            VoteObjectType::Entity,
            VoteValue::Up,
            1000,
        );
        let vote2 = make_vote_item(
            voter,
            object,
            space,
            VoteObjectType::Entity,
            VoteValue::Down,
            2000,
        );

        let votes = vec![vote1, vote2];
        let user_votes = get_latest_user_votes(&votes);

        // Should only return one vote (the latest one)
        assert_eq!(user_votes.len(), 1);
        assert_eq!(user_votes[0].vote_type, VoteValue::Down);
        assert_eq!(user_votes[0].voted_at, 2000);
    }

    #[test]
    fn test_get_latest_user_votes_different_users_same_entity() {
        let voter1 = Uuid::from_bytes([1u8; 16]);
        let voter2 = Uuid::from_bytes([2u8; 16]);
        let object = Uuid::from_bytes([3u8; 16]);
        let space = Uuid::from_bytes([4u8; 16]);

        let vote1 = make_vote_item(
            voter1,
            object,
            space,
            VoteObjectType::Entity,
            VoteValue::Up,
            1000,
        );
        let vote2 = make_vote_item(
            voter2,
            object,
            space,
            VoteObjectType::Entity,
            VoteValue::Down,
            2000,
        );

        let votes = vec![vote1, vote2];
        let user_votes = get_latest_user_votes(&votes);

        // Should return both votes since they're from different users
        assert_eq!(user_votes.len(), 2);
    }

    #[test]
    fn test_get_latest_user_votes_same_user_different_object_types() {
        let voter = Uuid::from_bytes([1u8; 16]);
        let object = Uuid::from_bytes([2u8; 16]);
        let space = Uuid::from_bytes([3u8; 16]);

        let vote1 = make_vote_item(
            voter,
            object,
            space,
            VoteObjectType::Entity,
            VoteValue::Up,
            1000,
        );
        let vote2 = make_vote_item(
            voter,
            object,
            space,
            VoteObjectType::Relation,
            VoteValue::Down,
            2000,
        );

        let votes = vec![vote1, vote2];
        let user_votes = get_latest_user_votes(&votes);

        // Should return both votes since they're for different object types
        assert_eq!(user_votes.len(), 2);
    }

    // ============================================================================
    // compute_vote_delta Tests
    // ============================================================================

    fn make_user_vote_item(vote_type: VoteValue) -> UserVoteItem {
        UserVoteItem {
            voter_id: Uuid::from_bytes([1u8; 16]),
            object_id: Uuid::from_bytes([2u8; 16]),
            object_type: VoteObjectType::Entity,
            space_id: Uuid::from_bytes([3u8; 16]),
            vote_type,
            kind: ResponseKind::Curation,
            voted_at: 1000,
        }
    }

    #[test]
    fn test_compute_vote_delta_upvote_to_downvote() {
        let prev = make_user_vote_item(VoteValue::Up);
        let new = make_user_vote_item(VoteValue::Down);

        let delta = compute_vote_delta(Some(&prev), &new);

        assert_eq!(
            delta,
            VotesDelta {
                positive: -1,
                negative: 1
            }
        );
    }

    #[test]
    fn test_compute_vote_delta_upvote_to_remove() {
        let prev = make_user_vote_item(VoteValue::Up);
        let new = make_user_vote_item(VoteValue::Remove);

        let delta = compute_vote_delta(Some(&prev), &new);

        assert_eq!(
            delta,
            VotesDelta {
                positive: -1,
                negative: 0
            }
        );
    }

    #[test]
    fn test_compute_vote_delta_downvote_to_upvote() {
        let prev = make_user_vote_item(VoteValue::Down);
        let new = make_user_vote_item(VoteValue::Up);

        let delta = compute_vote_delta(Some(&prev), &new);

        assert_eq!(
            delta,
            VotesDelta {
                positive: 1,
                negative: -1
            }
        );
    }

    #[test]
    fn test_compute_vote_delta_downvote_to_remove() {
        let prev = make_user_vote_item(VoteValue::Down);
        let new = make_user_vote_item(VoteValue::Remove);

        let delta = compute_vote_delta(Some(&prev), &new);

        assert_eq!(
            delta,
            VotesDelta {
                positive: 0,
                negative: -1
            }
        );
    }

    #[test]
    fn test_compute_vote_delta_new_upvote() {
        let new = make_user_vote_item(VoteValue::Up);

        let delta = compute_vote_delta(None, &new);

        assert_eq!(
            delta,
            VotesDelta {
                positive: 1,
                negative: 0
            }
        );
    }

    #[test]
    fn test_compute_vote_delta_new_downvote() {
        let new = make_user_vote_item(VoteValue::Down);

        let delta = compute_vote_delta(None, &new);

        assert_eq!(
            delta,
            VotesDelta {
                positive: 0,
                negative: 1
            }
        );
    }

    #[test]
    fn test_compute_vote_delta_same_vote_no_change() {
        let prev = make_user_vote_item(VoteValue::Up);
        let new = make_user_vote_item(VoteValue::Up);

        let delta = compute_vote_delta(Some(&prev), &new);

        assert_eq!(
            delta,
            VotesDelta {
                positive: 0,
                negative: 0
            }
        );
    }

    // ============================================================================
    // calculate_vote_counts Tests
    // ============================================================================

    #[test]
    fn test_calculate_vote_counts_new_upvote_no_existing() {
        let voter = Uuid::from_bytes([1u8; 16]);
        let object = Uuid::from_bytes([2u8; 16]);
        let space = Uuid::from_bytes([3u8; 16]);

        let user_votes = vec![UserVoteItem {
            voter_id: voter,
            object_id: object,
            object_type: VoteObjectType::Entity,
            space_id: space,
            vote_type: VoteValue::Up,
            kind: ResponseKind::Curation,
            voted_at: 1000,
        }];

        let stored_user_votes: HashMap<UserVoteCriteria, UserVoteItem> = HashMap::new();
        let stored_vote_counts: HashMap<VoteCountCriteria, VotesCountItem> = HashMap::new();

        let counts = calculate_vote_counts(&user_votes, &stored_user_votes, &stored_vote_counts);

        assert_eq!(counts.len(), 1);
        assert_eq!(counts[0].positive, 1);
        assert_eq!(counts[0].negative, 0);
    }

    #[test]
    fn test_calculate_vote_counts_change_upvote_to_downvote() {
        let voter = Uuid::from_bytes([1u8; 16]);
        let object = Uuid::from_bytes([2u8; 16]);
        let space = Uuid::from_bytes([3u8; 16]);

        let user_votes = vec![UserVoteItem {
            voter_id: voter,
            object_id: object,
            object_type: VoteObjectType::Entity,
            space_id: space,
            vote_type: VoteValue::Down,
            kind: ResponseKind::Curation,
            voted_at: 2000,
        }];

        let mut stored_user_votes: HashMap<UserVoteCriteria, UserVoteItem> = HashMap::new();
        stored_user_votes.insert(
            (
                voter,
                object,
                space,
                VoteObjectType::Entity,
                ResponseKind::Curation,
            ),
            UserVoteItem {
                voter_id: voter,
                object_id: object,
                object_type: VoteObjectType::Entity,
                space_id: space,
                vote_type: VoteValue::Up,
                kind: ResponseKind::Curation,
                voted_at: 1000,
            },
        );

        let mut stored_vote_counts: HashMap<VoteCountCriteria, VotesCountItem> = HashMap::new();
        stored_vote_counts.insert(
            (
                object,
                space,
                VoteObjectType::Entity,
                ResponseKind::Curation,
            ),
            VotesCountItem {
                object_id: object,
                object_type: VoteObjectType::Entity,
                space_id: space,
                kind: ResponseKind::Curation,
                positive: 5,
                negative: 2,
            },
        );

        let counts = calculate_vote_counts(&user_votes, &stored_user_votes, &stored_vote_counts);

        assert_eq!(counts.len(), 1);
        assert_eq!(counts[0].positive, 4); // 5 - 1
        assert_eq!(counts[0].negative, 3); // 2 + 1
    }

    #[test]
    fn test_calculate_vote_counts_multiple_users_same_object() {
        let voter1 = Uuid::from_bytes([1u8; 16]);
        let voter2 = Uuid::from_bytes([2u8; 16]);
        let object = Uuid::from_bytes([3u8; 16]);
        let space = Uuid::from_bytes([4u8; 16]);

        let user_votes = vec![
            UserVoteItem {
                voter_id: voter1,
                object_id: object,
                object_type: VoteObjectType::Entity,
                space_id: space,
                vote_type: VoteValue::Up,
                kind: ResponseKind::Curation,
                voted_at: 1000,
            },
            UserVoteItem {
                voter_id: voter2,
                object_id: object,
                object_type: VoteObjectType::Entity,
                space_id: space,
                vote_type: VoteValue::Down,
                kind: ResponseKind::Curation,
                voted_at: 1000,
            },
        ];

        let stored_user_votes: HashMap<UserVoteCriteria, UserVoteItem> = HashMap::new();
        let stored_vote_counts: HashMap<VoteCountCriteria, VotesCountItem> = HashMap::new();

        let counts = calculate_vote_counts(&user_votes, &stored_user_votes, &stored_vote_counts);

        assert_eq!(counts.len(), 1);
        assert_eq!(counts[0].positive, 1);
        assert_eq!(counts[0].negative, 1);
    }

    // ============================================================================
    // build_score_values Tests
    // ============================================================================

    #[test]
    fn test_build_score_values_entity_net_score() {
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();
        let counts = vec![VotesCountItem {
            object_id: entity_id,
            object_type: VoteObjectType::Entity,
            space_id,
            kind: ResponseKind::Curation,
            positive: 5,
            negative: 2,
        }];

        let rows = build_score_values(&counts);

        let ns = Uuid::parse_str(GEO_SYSTEM_NAMESPACE).unwrap();
        let expected_id = derive_score_value_id(&ns, &entity_id, &space_id);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, expected_id);
        assert_eq!(rows[0].entity_id, entity_id);
        assert_eq!(rows[0].space_id, space_id);
        assert_eq!(rows[0].integer, 3);
    }

    #[test]
    fn test_build_score_values_id_is_deterministic() {
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();
        let count = VotesCountItem {
            object_id: entity_id,
            object_type: VoteObjectType::Entity,
            space_id,
            kind: ResponseKind::Curation,
            positive: 1,
            negative: 0,
        };

        let first = build_score_values(&[count.clone()]);
        let second = build_score_values(&[count]);

        assert_eq!(first[0].id, second[0].id);
    }

    #[test]
    fn test_build_score_values_negative_score() {
        let counts = vec![VotesCountItem {
            object_id: Uuid::new_v4(),
            object_type: VoteObjectType::Entity,
            space_id: Uuid::new_v4(),
            kind: ResponseKind::Curation,
            positive: 1,
            negative: 4,
        }];

        let rows = build_score_values(&counts);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].integer, -3);
    }

    #[test]
    fn test_build_score_values_filters_out_relations() {
        let counts = vec![
            VotesCountItem {
                object_id: Uuid::new_v4(),
                object_type: VoteObjectType::Relation,
                space_id: Uuid::new_v4(),
                kind: ResponseKind::Curation,
                positive: 10,
                negative: 0,
            },
            VotesCountItem {
                object_id: Uuid::new_v4(),
                object_type: VoteObjectType::Entity,
                space_id: Uuid::new_v4(),
                kind: ResponseKind::Curation,
                positive: 2,
                negative: 1,
            },
        ];

        let rows = build_score_values(&counts);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].integer, 1);
    }

    #[test]
    fn test_build_score_values_empty_input() {
        let rows = build_score_values(&[]);
        assert!(rows.is_empty());
    }

    #[test]
    fn test_build_score_values_preserves_per_space_rows() {
        let entity_id = Uuid::new_v4();
        let space_a = Uuid::new_v4();
        let space_b = Uuid::new_v4();
        let counts = vec![
            VotesCountItem {
                object_id: entity_id,
                object_type: VoteObjectType::Entity,
                space_id: space_a,
                kind: ResponseKind::Curation,
                positive: 3,
                negative: 0,
            },
            VotesCountItem {
                object_id: entity_id,
                object_type: VoteObjectType::Entity,
                space_id: space_b,
                kind: ResponseKind::Curation,
                positive: 0,
                negative: 5,
            },
        ];

        let rows = build_score_values(&counts);

        let ns = Uuid::parse_str(GEO_SYSTEM_NAMESPACE).unwrap();
        assert_eq!(rows.len(), 2);
        let row_a = rows.iter().find(|r| r.space_id == space_a).unwrap();
        let row_b = rows.iter().find(|r| r.space_id == space_b).unwrap();
        assert_eq!(row_a.integer, 3);
        assert_eq!(row_a.id, derive_score_value_id(&ns, &entity_id, &space_a));
        assert_eq!(row_b.integer, -5);
        assert_eq!(row_b.id, derive_score_value_id(&ns, &entity_id, &space_b));
        assert_ne!(row_a.id, row_b.id);
    }

    // ========================================================================
    // vote_kind: the axes must stay independent
    //
    // The failure these cover is silent — no error, no log, just a vote that
    // stops existing or a tally that drifts. They are the indexer-side half of
    // what the widened unique constraint provides.
    // ========================================================================

    #[test]
    fn handle_vote_cast_decodes_each_kind() {
        for (proto_kind, want) in [
            (VoteKind::Curation, ResponseKind::Curation),
            (VoteKind::Stance, ResponseKind::Stance),
            (VoteKind::Veracity, ResponseKind::Veracity),
        ] {
            let vote = HermesVoteCast {
                voter_id: make_test_uuid(),
                object_type: OBJECT_TYPE_ENTITY.to_vec(),
                object_id: make_test_uuid(),
                direction: VoteDirection::Up as i32,
                version: 1,
                group_id: make_test_uuid(),
                space_pov: make_test_uuid(),
                meta: Some(make_test_meta()),
                kind: proto_kind as i32,
            };
            assert_eq!(handle_vote_cast(&vote).unwrap().kind, want);
        }
    }

    /// An unknown kind from a newer producer must not halt the consumer, and
    /// must NOT be folded into curation — that would key it identically to the
    /// user's real curation vote and overwrite it, which is the silent loss
    /// vote_kind exists to prevent.
    #[test]
    fn handle_vote_cast_unknown_kind_preserves_its_discriminant() {
        let vote = HermesVoteCast {
            voter_id: make_test_uuid(),
            object_type: OBJECT_TYPE_ENTITY.to_vec(),
            object_id: make_test_uuid(),
            direction: VoteDirection::Up as i32,
            version: 1,
            group_id: make_test_uuid(),
            space_pov: make_test_uuid(),
            meta: Some(make_test_meta()),
            kind: 99,
        };
        let decoded = handle_vote_cast(&vote).unwrap().kind;
        assert_eq!(decoded, ResponseKind::Unknown(99));
        assert_ne!(decoded, ResponseKind::Curation);
        // Round-trips through the DB column type without collapsing to 0.
        assert_eq!(i16::from(decoded), 99);
    }

    /// The dedup key must treat an unknown kind as its own axis, so a future
    /// kind cannot clobber an existing curation vote.
    #[test]
    fn unknown_kind_does_not_collide_with_curation() {
        let voter = Uuid::from_bytes([1u8; 16]);
        let object = Uuid::from_bytes([2u8; 16]);
        let space = Uuid::from_bytes([3u8; 16]);

        let votes = vec![
            make_kinded_vote_item(
                voter,
                object,
                space,
                VoteObjectType::Entity,
                VoteValue::Up,
                ResponseKind::Curation,
                1000,
            ),
            make_kinded_vote_item(
                voter,
                object,
                space,
                VoteObjectType::Entity,
                VoteValue::Down,
                ResponseKind::Unknown(99),
                2000,
            ),
        ];

        // Two distinct responses, not one overwriting the other.
        let user_votes = get_latest_user_votes(&votes);
        assert_eq!(user_votes.len(), 2);

        let curation = user_votes
            .iter()
            .find(|v| v.kind == ResponseKind::Curation)
            .expect("curation row must survive an unknown-kind vote");
        assert_eq!(curation.vote_type, VoteValue::Up);
    }

    /// Same user, same object, same block, different axes — three responses,
    /// not one. If the dedup key dropped the kind, two would be discarded.
    #[test]
    fn get_latest_user_votes_keeps_one_row_per_kind() {
        let voter = Uuid::from_bytes([1u8; 16]);
        let object = Uuid::from_bytes([2u8; 16]);
        let space = Uuid::from_bytes([3u8; 16]);

        let votes: Vec<VoteItem> = [
            ResponseKind::Curation,
            ResponseKind::Stance,
            ResponseKind::Veracity,
        ]
        .into_iter()
        .map(|k| {
            make_kinded_vote_item(
                voter,
                object,
                space,
                VoteObjectType::Entity,
                VoteValue::Up,
                k,
                1000,
            )
        })
        .collect();

        let user_votes = get_latest_user_votes(&votes);
        assert_eq!(user_votes.len(), 3);

        let mut kinds: Vec<i16> = user_votes.iter().map(|v| i16::from(v.kind)).collect();
        kinds.sort_unstable();
        assert_eq!(kinds, vec![0, 1, 2]);
    }

    /// Within one kind, the latest response still wins.
    #[test]
    fn get_latest_user_votes_still_dedups_within_a_kind() {
        let voter = Uuid::from_bytes([1u8; 16]);
        let object = Uuid::from_bytes([2u8; 16]);
        let space = Uuid::from_bytes([3u8; 16]);

        let votes = vec![
            make_kinded_vote_item(
                voter,
                object,
                space,
                VoteObjectType::Entity,
                VoteValue::Up,
                ResponseKind::Veracity,
                1000,
            ),
            make_kinded_vote_item(
                voter,
                object,
                space,
                VoteObjectType::Entity,
                VoteValue::Down,
                ResponseKind::Veracity,
                2000,
            ),
        ];

        let user_votes = get_latest_user_votes(&votes);
        assert_eq!(user_votes.len(), 1);
        assert_eq!(user_votes[0].vote_type, VoteValue::Down);
        assert_eq!(user_votes[0].kind, ResponseKind::Veracity);
    }

    /// THE regression test for the indexer: casting a Verify on an object the
    /// user already upvoted must leave the curation tally untouched.
    #[test]
    fn verify_does_not_disturb_an_existing_upvote() {
        let voter = Uuid::from_bytes([1u8; 16]);
        let object = Uuid::from_bytes([2u8; 16]);
        let space = Uuid::from_bytes([3u8; 16]);

        // Stored state: the user has an upvote; curation tally is 5/2.
        let mut stored_user_votes: HashMap<UserVoteCriteria, UserVoteItem> = HashMap::new();
        stored_user_votes.insert(
            (
                voter,
                object,
                space,
                VoteObjectType::Entity,
                ResponseKind::Curation,
            ),
            UserVoteItem {
                voter_id: voter,
                object_id: object,
                object_type: VoteObjectType::Entity,
                space_id: space,
                vote_type: VoteValue::Up,
                kind: ResponseKind::Curation,
                voted_at: 1000,
            },
        );

        let mut stored_vote_counts: HashMap<VoteCountCriteria, VotesCountItem> = HashMap::new();
        stored_vote_counts.insert(
            (
                object,
                space,
                VoteObjectType::Entity,
                ResponseKind::Curation,
            ),
            VotesCountItem {
                object_id: object,
                object_type: VoteObjectType::Entity,
                space_id: space,
                kind: ResponseKind::Curation,
                positive: 5,
                negative: 2,
            },
        );

        // The same user now casts a Verify.
        let new_votes = vec![UserVoteItem {
            voter_id: voter,
            object_id: object,
            object_type: VoteObjectType::Entity,
            space_id: space,
            vote_type: VoteValue::Up,
            kind: ResponseKind::Veracity,
            voted_at: 2000,
        }];

        let counts = calculate_vote_counts(&new_votes, &stored_user_votes, &stored_vote_counts);

        let curation = counts
            .iter()
            .find(|c| c.kind == ResponseKind::Curation)
            .expect("curation row must survive");
        assert_eq!(
            curation.positive, 5,
            "Verify must not move the upvote tally"
        );
        assert_eq!(curation.negative, 2);

        let veracity = counts
            .iter()
            .find(|c| c.kind == ResponseKind::Veracity)
            .expect("veracity row must be created");
        assert_eq!(veracity.positive, 1);
        assert_eq!(veracity.negative, 0);
    }

    /// A kind-scoped clear removes only its own axis. An UNVERIFIED from a user
    /// holding both an upvote and a verification must zero the veracity tally
    /// and leave curation alone.
    #[test]
    fn clear_is_scoped_to_its_own_kind() {
        let voter = Uuid::from_bytes([1u8; 16]);
        let object = Uuid::from_bytes([2u8; 16]);
        let space = Uuid::from_bytes([3u8; 16]);

        let mut stored_user_votes: HashMap<UserVoteCriteria, UserVoteItem> = HashMap::new();
        for kind in [ResponseKind::Curation, ResponseKind::Veracity] {
            stored_user_votes.insert(
                (voter, object, space, VoteObjectType::Entity, kind),
                UserVoteItem {
                    voter_id: voter,
                    object_id: object,
                    object_type: VoteObjectType::Entity,
                    space_id: space,
                    vote_type: VoteValue::Up,
                    kind,
                    voted_at: 1000,
                },
            );
        }

        let mut stored_vote_counts: HashMap<VoteCountCriteria, VotesCountItem> = HashMap::new();
        for kind in [ResponseKind::Curation, ResponseKind::Veracity] {
            stored_vote_counts.insert(
                (object, space, VoteObjectType::Entity, kind),
                VotesCountItem {
                    object_id: object,
                    object_type: VoteObjectType::Entity,
                    space_id: space,
                    kind,
                    positive: 4,
                    negative: 0,
                },
            );
        }

        // UNVERIFIED: clear, on the veracity axis only.
        let new_votes = vec![UserVoteItem {
            voter_id: voter,
            object_id: object,
            object_type: VoteObjectType::Entity,
            space_id: space,
            vote_type: VoteValue::Remove,
            kind: ResponseKind::Veracity,
            voted_at: 2000,
        }];

        let counts = calculate_vote_counts(&new_votes, &stored_user_votes, &stored_vote_counts);

        let curation = counts
            .iter()
            .find(|c| c.kind == ResponseKind::Curation)
            .unwrap();
        let veracity = counts
            .iter()
            .find(|c| c.kind == ResponseKind::Veracity)
            .unwrap();

        assert_eq!(curation.positive, 4, "UNVERIFIED must not clear the upvote");
        assert_eq!(veracity.positive, 3, "veracity tally must drop by one");
    }

    /// Score mirroring feeds ranking, which stays curation-only. Non-curation
    /// rows share the same (entity, space) value id, so without this filter the
    /// last kind processed would silently overwrite the entity's score.
    #[test]
    fn build_score_values_ignores_non_curation_kinds() {
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let counts = vec![
            VotesCountItem {
                object_id: entity_id,
                object_type: VoteObjectType::Entity,
                space_id,
                kind: ResponseKind::Curation,
                positive: 3,
                negative: 1,
            },
            VotesCountItem {
                object_id: entity_id,
                object_type: VoteObjectType::Entity,
                space_id,
                kind: ResponseKind::Veracity,
                positive: 900,
                negative: 0,
            },
            VotesCountItem {
                object_id: entity_id,
                object_type: VoteObjectType::Entity,
                space_id,
                kind: ResponseKind::Stance,
                positive: 500,
                negative: 0,
            },
        ];

        let rows = build_score_values(&counts);

        assert_eq!(rows.len(), 1, "only the curation row may mirror a score");
        assert_eq!(rows[0].integer, 2);
    }
}
