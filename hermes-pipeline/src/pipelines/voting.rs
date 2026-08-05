//! Pipeline: the nine permissionless response actions → curation.votes
//!
//! Converts permissionless response actions to typed Hermes events with decoded
//! data. Three kinds — curation (upvote/downvote), stance (agree/disagree) and
//! veracity (verify/dispute) — each with a positive, a negative and a clear
//! action. Topic and data encoding are identical across all nine; the action
//! hash is the only thing that distinguishes them, so this module is where a
//! hash becomes a `(kind, direction)` pair.

use anyhow::Result;
use hermes_instrumentation::{debug_span, warn};

use hermes_relay::{Action, actions};
use hermes_schema::pb::voting::{HermesVoteCast, VoteDirection, VoteKind};

use crate::decode;

use super::BlockMetadata;

/// Result of transforming response actions.
///
/// Counted on both axes: `positive`/`negative`/`clear` are direction totals
/// across every kind, and `curation`/`stance`/`veracity` are per-kind totals.
/// The per-kind counters are what tell an operator that the first stance or
/// veracity events have actually started arriving after registration.
#[derive(Debug, Default)]
pub struct TransformResult {
    pub votes: Vec<HermesVoteCast>,
    pub positive: u64,
    pub negative: u64,
    pub clear: u64,
    pub curation: u64,
    pub stance: u64,
    pub veracity: u64,
}

impl TransformResult {
    pub fn total(&self) -> usize {
        self.votes.len()
    }
}

/// Resolve a 32-byte action hash to the response it represents.
///
/// Returns `None` for any action that is not one of the nine — the pipeline
/// hands every action in the block to every transform, so non-response actions
/// falling through here is the normal case, not an error.
///
/// The clear actions are deliberately per-kind (`UNVOTED` / `UNAGREED` /
/// `UNVERIFIED`) rather than one shared clear. Because a user may hold a
/// response of each kind at once, a shared clear would be ambiguous — the
/// indexer could not tell which of their rows to remove.
fn resolve_response(action_type: &[u8]) -> Option<(VoteKind, VoteDirection)> {
    // Curation.
    if actions::matches(action_type, &actions::UPVOTED) {
        Some((VoteKind::Curation, VoteDirection::Up))
    } else if actions::matches(action_type, &actions::DOWNVOTED) {
        Some((VoteKind::Curation, VoteDirection::Down))
    } else if actions::matches(action_type, &actions::UNVOTED) {
        Some((VoteKind::Curation, VoteDirection::None))
    // Stance.
    } else if actions::matches(action_type, &actions::AGREED) {
        Some((VoteKind::Stance, VoteDirection::Up))
    } else if actions::matches(action_type, &actions::DISAGREED) {
        Some((VoteKind::Stance, VoteDirection::Down))
    } else if actions::matches(action_type, &actions::UNAGREED) {
        Some((VoteKind::Stance, VoteDirection::None))
    // Veracity.
    } else if actions::matches(action_type, &actions::VERIFIED) {
        Some((VoteKind::Veracity, VoteDirection::Up))
    } else if actions::matches(action_type, &actions::DISPUTED) {
        Some((VoteKind::Veracity, VoteDirection::Down))
    } else if actions::matches(action_type, &actions::UNVERIFIED) {
        Some((VoteKind::Veracity, VoteDirection::None))
    } else {
        None
    }
}

/// Static span name for a `(kind, direction)` pair.
///
/// `debug_span!` requires a literal name, so this maps to one of nine consts
/// rather than formatting a string.
fn span_name(kind: VoteKind, direction: VoteDirection) -> &'static str {
    match (kind, direction) {
        (VoteKind::Curation, VoteDirection::Up) => "convert.voting.upvoted",
        (VoteKind::Curation, VoteDirection::Down) => "convert.voting.downvoted",
        (VoteKind::Curation, VoteDirection::None) => "convert.voting.unvoted",
        (VoteKind::Stance, VoteDirection::Up) => "convert.voting.agreed",
        (VoteKind::Stance, VoteDirection::Down) => "convert.voting.disagreed",
        (VoteKind::Stance, VoteDirection::None) => "convert.voting.unagreed",
        (VoteKind::Veracity, VoteDirection::Up) => "convert.voting.verified",
        (VoteKind::Veracity, VoteDirection::Down) => "convert.voting.disputed",
        (VoteKind::Veracity, VoteDirection::None) => "convert.voting.unverified",
    }
}

/// Transform all response actions in a block.
///
/// Returns transformed events without sending to Kafka.
pub fn transform(actions: &[Action], meta: &BlockMetadata) -> Result<TransformResult> {
    let mut result = TransformResult::default();

    for (index, action) in actions.iter().enumerate() {
        let action_type = action.action.as_slice();
        let sequence = index as u32;

        let Some((kind, direction)) = resolve_response(action_type) else {
            continue;
        };

        // Slice defensively: a malformed topic must not panic the pipeline.
        // `convert_vote` already tolerates a short topic; the span has to as well.
        let object_id_hex = action
            .topic
            .get(4..20)
            .map(hex::encode)
            .unwrap_or_else(|| "<malformed>".to_string());

        let event = debug_span!(
            "convert.voting",
            response = span_name(kind, direction),
            voter_id = %hex::encode(&action.from_id),
            object_id = %object_id_hex
        )
        .in_scope(|| convert_vote(action, meta, kind, direction, sequence))?;
        result.votes.push(event);

        match direction {
            VoteDirection::Up => result.positive += 1,
            VoteDirection::Down => result.negative += 1,
            VoteDirection::None => result.clear += 1,
        }
        match kind {
            VoteKind::Curation => result.curation += 1,
            VoteKind::Stance => result.stance += 1,
            VoteKind::Veracity => result.veracity += 1,
        }
    }

    Ok(result)
}

/// Convert any of the nine response actions to a HermesVoteCast proto.
///
/// The action structure is identical for all nine — only the hash differs, and
/// the caller has already resolved that into `kind` + `direction`:
/// - from_id: voter_id (16 bytes) - voter's space ID
/// - topic: bytes32(bytes4(objectType) << 224) | (bytes16(objectId) << 96)
///   - topic[0..4]: object type (4 bytes)
///   - topic[4..20]: object ID (16 bytes)
/// - data: abi.encode(uint16(version), bytes16(groupId), bytes16(spacePOV))
///
/// Note `object_type` (entity vs relation) and `kind` are orthogonal
/// dimensions: object_type says *what* was responded to, kind says *which
/// axis* the response is on.
fn convert_vote(
    action: &Action,
    meta: &BlockMetadata,
    kind: VoteKind,
    direction: VoteDirection,
    sequence: u32,
) -> Result<HermesVoteCast> {
    // Extract object type and ID from topic
    let object_type = if action.topic.len() >= 4 {
        action.topic[0..4].to_vec()
    } else {
        vec![0; 4]
    };

    let object_id = if action.topic.len() >= 20 {
        action.topic[4..20].to_vec()
    } else {
        vec![0; 16]
    };

    // Decode the data field
    let (version, group_id, space_pov) = match decode::decode_vote_data(&action.data) {
        Ok(decoded) => (decoded.version as u32, decoded.group_id, decoded.space_pov),
        Err(e) => {
            warn!(
                error = %e,
                voter_id = %hex::encode(&action.from_id),
                "Failed to decode vote data"
            );
            (0, vec![0; 16], vec![0; 16])
        }
    };

    Ok(HermesVoteCast {
        voter_id: action.from_id.clone(),
        object_type,
        object_id,
        direction: direction as i32,
        version,
        group_id,
        space_pov,
        meta: Some(meta.to_proto(sequence)),
        kind: kind as i32,
    })
}

/// Helper to get vote direction string for logging
pub fn get_vote_direction(vote: &HermesVoteCast) -> &'static str {
    match VoteDirection::try_from(vote.direction) {
        Ok(VoteDirection::Up) => "UP",
        Ok(VoteDirection::Down) => "DOWN",
        Ok(VoteDirection::None) => "NONE",
        Err(_) => "UNKNOWN",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_meta() -> BlockMetadata {
        BlockMetadata {
            cursor: "test_cursor".to_string(),
            block_number: 12345,
            timestamp: "1234567890".to_string(),
        }
    }

    fn make_vote_topic(object_type: [u8; 4], object_id: [u8; 16]) -> Vec<u8> {
        let mut topic = vec![0u8; 32];
        topic[0..4].copy_from_slice(&object_type);
        topic[4..20].copy_from_slice(&object_id);
        topic
    }

    #[test]
    fn test_convert_upvote_empty_data() {
        let object_type = [0x00, 0x00, 0x00, 0x01];
        let object_id = [2u8; 16];

        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::UPVOTED.to_vec(),
            topic: make_vote_topic(object_type, object_id),
            data: vec![],
        };

        let result = convert_vote(
            &action,
            &test_meta(),
            VoteKind::Curation,
            VoteDirection::Up,
            0,
        )
        .unwrap();
        assert_eq!(result.voter_id, vec![1; 16]);
        assert_eq!(result.object_type, object_type.to_vec());
        assert_eq!(result.object_id, object_id.to_vec());
        assert_eq!(result.direction, VoteDirection::Up as i32);
        // Default values when decode fails
        assert_eq!(result.version, 0);
        assert_eq!(result.group_id.len(), 16);
        assert_eq!(result.space_pov.len(), 16);
    }

    #[test]
    fn test_convert_downvote_empty_data() {
        let object_type = [0x00, 0x00, 0x00, 0x02];
        let object_id = [5u8; 16];
        let action = Action {
            from_id: vec![4; 16],
            to_id: vec![],
            action: actions::DOWNVOTED.to_vec(),
            topic: make_vote_topic(object_type, object_id),
            data: vec![],
        };

        let result = convert_vote(
            &action,
            &test_meta(),
            VoteKind::Curation,
            VoteDirection::Down,
            0,
        )
        .unwrap();
        assert_eq!(result.direction, VoteDirection::Down as i32);
        assert_eq!(result.version, 0);
        assert_eq!(result.group_id.len(), 16);
        assert_eq!(result.space_pov.len(), 16);
    }

    #[test]
    fn test_convert_unvote() {
        let object_type = [0x00, 0x00, 0x00, 0x01];
        let object_id = [6u8; 16];
        let action = Action {
            from_id: vec![7; 16],
            to_id: vec![],
            action: actions::UNVOTED.to_vec(),
            topic: make_vote_topic(object_type, object_id),
            data: vec![],
        };

        let result = convert_vote(
            &action,
            &test_meta(),
            VoteKind::Curation,
            VoteDirection::None,
            0,
        )
        .unwrap();
        assert_eq!(result.direction, VoteDirection::None as i32);
    }

    #[test]
    fn test_transform_counts() {
        let object_type = [0x00, 0x00, 0x00, 0x01];
        let actions = vec![
            Action {
                from_id: vec![1; 16],
                to_id: vec![],
                action: actions::UPVOTED.to_vec(),
                topic: make_vote_topic(object_type, [1u8; 16]),
                data: vec![],
            },
            Action {
                from_id: vec![2; 16],
                to_id: vec![],
                action: actions::UPVOTED.to_vec(),
                topic: make_vote_topic(object_type, [2u8; 16]),
                data: vec![],
            },
            Action {
                from_id: vec![3; 16],
                to_id: vec![],
                action: actions::DOWNVOTED.to_vec(),
                topic: make_vote_topic(object_type, [3u8; 16]),
                data: vec![],
            },
            Action {
                from_id: vec![4; 16],
                to_id: vec![],
                action: actions::UNVOTED.to_vec(),
                topic: make_vote_topic(object_type, [4u8; 16]),
                data: vec![],
            },
            // Should NOT be included
            Action {
                from_id: vec![5; 16],
                to_id: vec![],
                action: actions::SPACE_REGISTERED.to_vec(),
                topic: vec![6; 32],
                data: vec![],
            },
        ];

        let result = transform(&actions, &test_meta()).unwrap();
        assert_eq!(result.total(), 4);
        assert_eq!(result.positive, 2);
        assert_eq!(result.negative, 1);
        assert_eq!(result.clear, 1);
        assert_eq!(result.curation, 4);
        assert_eq!(result.stance, 0);
        assert_eq!(result.veracity, 0);
    }

    #[test]
    fn test_get_vote_direction() {
        let vote = HermesVoteCast {
            voter_id: vec![],
            object_type: vec![],
            object_id: vec![],
            direction: VoteDirection::Up as i32,
            version: 0,
            group_id: vec![],
            space_pov: vec![],
            meta: None,
            kind: VoteKind::Curation as i32,
        };
        assert_eq!(get_vote_direction(&vote), "UP");
    }

    // ========================================================================
    // vote_kind: the six new response actions
    // ========================================================================

    /// Every one of the nine hashes resolves to the intended (kind, direction).
    ///
    /// This is the table that decides what gets written to the database, so it
    /// is asserted exhaustively rather than by sampling.
    #[test]
    fn resolve_response_maps_all_nine_actions() {
        let cases: [(&[u8; 32], VoteKind, VoteDirection); 9] = [
            (&actions::UPVOTED, VoteKind::Curation, VoteDirection::Up),
            (&actions::DOWNVOTED, VoteKind::Curation, VoteDirection::Down),
            (&actions::UNVOTED, VoteKind::Curation, VoteDirection::None),
            (&actions::AGREED, VoteKind::Stance, VoteDirection::Up),
            (&actions::DISAGREED, VoteKind::Stance, VoteDirection::Down),
            (&actions::UNAGREED, VoteKind::Stance, VoteDirection::None),
            (&actions::VERIFIED, VoteKind::Veracity, VoteDirection::Up),
            (&actions::DISPUTED, VoteKind::Veracity, VoteDirection::Down),
            (
                &actions::UNVERIFIED,
                VoteKind::Veracity,
                VoteDirection::None,
            ),
        ];

        for (hash, want_kind, want_direction) in cases {
            let got = resolve_response(hash);
            assert_eq!(
                got,
                Some((want_kind, want_direction)),
                "hash {} resolved to {:?}",
                hex::encode(hash),
                got
            );
        }
    }

    /// A non-response action must not be picked up as one. The pipeline feeds
    /// every action in the block to this transform, so a too-loose match would
    /// mint bogus votes from unrelated governance events.
    #[test]
    fn resolve_response_ignores_non_response_actions() {
        assert_eq!(resolve_response(&actions::SPACE_REGISTERED), None);
        assert_eq!(resolve_response(&actions::EDITOR_ADDED), None);
        assert_eq!(resolve_response(&actions::PROPOSAL_VOTED), None);
        // GOVERNANCE.SUBSPACE_VERIFIED shares a word with PERMISSIONLESS.VERIFIED
        // but is an entirely different action.
        assert_eq!(resolve_response(&actions::SUBSPACE_VERIFIED), None);
    }

    /// The kind reaches the emitted proto — not just the direction.
    #[test]
    fn transform_stamps_kind_on_events() {
        let object_type = [0x00, 0x00, 0x00, 0x00];
        let cases = [
            (actions::AGREED, VoteKind::Stance, VoteDirection::Up),
            (actions::DISPUTED, VoteKind::Veracity, VoteDirection::Down),
            (actions::UNVERIFIED, VoteKind::Veracity, VoteDirection::None),
        ];

        for (hash, want_kind, want_direction) in cases {
            let action = Action {
                from_id: vec![1; 16],
                to_id: vec![],
                action: hash.to_vec(),
                topic: make_vote_topic(object_type, [9u8; 16]),
                data: vec![],
            };
            let result = transform(&[action], &test_meta()).unwrap();
            assert_eq!(result.total(), 1);
            assert_eq!(result.votes[0].kind, want_kind as i32);
            assert_eq!(result.votes[0].direction, want_direction as i32);
        }
    }

    /// A block mixing all three kinds counts each on both axes independently.
    #[test]
    fn transform_counts_kinds_independently() {
        let object_type = [0x00, 0x00, 0x00, 0x00];
        let make = |hash: [u8; 32], n: u8| Action {
            from_id: vec![n; 16],
            to_id: vec![],
            action: hash.to_vec(),
            topic: make_vote_topic(object_type, [n; 16]),
            data: vec![],
        };

        let acts = vec![
            make(actions::UPVOTED, 1),
            make(actions::AGREED, 2),
            make(actions::DISAGREED, 3),
            make(actions::VERIFIED, 4),
            make(actions::DISPUTED, 5),
            make(actions::UNVERIFIED, 6),
        ];

        let result = transform(&acts, &test_meta()).unwrap();

        assert_eq!(result.total(), 6);
        // Direction axis.
        assert_eq!(result.positive, 3); // upvote, agree, verify
        assert_eq!(result.negative, 2); // disagree, dispute
        assert_eq!(result.clear, 1); // unverify
        // Kind axis.
        assert_eq!(result.curation, 1);
        assert_eq!(result.stance, 2);
        assert_eq!(result.veracity, 3);
    }

    /// A clear action carries the kind it clears, so the indexer can scope the
    /// delete. If every clear collapsed to one kind, an UNVERIFIED would wipe
    /// the user's curation vote instead of their verification.
    #[test]
    fn clear_actions_are_kind_scoped() {
        let object_type = [0x00, 0x00, 0x00, 0x00];
        let clears = [
            (actions::UNVOTED, VoteKind::Curation),
            (actions::UNAGREED, VoteKind::Stance),
            (actions::UNVERIFIED, VoteKind::Veracity),
        ];

        for (hash, want_kind) in clears {
            let action = Action {
                from_id: vec![1; 16],
                to_id: vec![],
                action: hash.to_vec(),
                topic: make_vote_topic(object_type, [1u8; 16]),
                data: vec![],
            };
            let result = transform(&[action], &test_meta()).unwrap();
            assert_eq!(result.votes[0].direction, VoteDirection::None as i32);
            assert_eq!(
                result.votes[0].kind, want_kind as i32,
                "clear action must name the kind it clears"
            );
        }
    }

    /// A malformed (short) topic must not panic the transform. Every action in
    /// the block flows through here, so a panic is a stalled pipeline.
    #[test]
    fn transform_tolerates_short_topic() {
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::VERIFIED.to_vec(),
            topic: vec![0u8; 8], // shorter than the 20 bytes a topic should hold
            data: vec![],
        };

        let result = transform(&[action], &test_meta()).unwrap();
        assert_eq!(result.total(), 1);
        assert_eq!(result.votes[0].object_id, vec![0u8; 16]);
    }
}
