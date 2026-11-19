//! Action type constants for filtering raw actions.
//!
//! When consuming `HermesModule::Actions`, use these constants to filter
//! for specific action types by comparing against the `action` field (32 bytes).
//!
//! These are re-exported from `hermes_substream` where the canonical definitions live.
//!
//! ## Action Name Format
//!
//! - Governance actions use 'GOVERNANCE.' prefix (e.g., "GOVERNANCE.EDITOR_ADDED")
//! - Permissionless actions use 'PERMISSIONLESS.' prefix (e.g., "PERMISSIONLESS.UPVOTED")
//!
//! ## Example
//!
//! ```ignore
//! use hermes_relay::actions;
//!
//! fn filter_space_events(action: &Action) -> bool {
//!     let action_type = action.action.as_slice();
//!     action_type == actions::SPACE_REGISTERED
//!         || action_type == actions::SUBSPACE_VERIFIED
//!         || action_type == actions::SUBSPACE_RELATED
//! }
//! ```

// Re-export action constants from hermes_substream with shorter names for convenience.
// The canonical definitions with full ACTION_* names are in hermes_substream::lib.

// =============================================================================
// Governance Actions
// =============================================================================

pub use hermes_substream::ACTION_EDITOR_ADDED as EDITOR_ADDED;
pub use hermes_substream::ACTION_EDITOR_REMOVED as EDITOR_REMOVED;
pub use hermes_substream::ACTION_EDITS_PUBLISHED as EDITS_PUBLISHED;
pub use hermes_substream::ACTION_FLAGGED as FLAGGED;
pub use hermes_substream::ACTION_MEMBER_ADDED as MEMBER_ADDED;
pub use hermes_substream::ACTION_MEMBER_REMOVED as MEMBER_REMOVED;
pub use hermes_substream::ACTION_MEMBERSHIP_REQUESTED as MEMBERSHIP_REQUESTED;
pub use hermes_substream::ACTION_PERMISSIONLESS_ACTION_ADDED as PERMISSIONLESS_ACTION_ADDED;
pub use hermes_substream::ACTION_PERMISSIONLESS_ACTION_REMOVED as PERMISSIONLESS_ACTION_REMOVED;
pub use hermes_substream::ACTION_PROPOSAL_CREATED as PROPOSAL_CREATED;
pub use hermes_substream::ACTION_PROPOSAL_EXECUTED as PROPOSAL_EXECUTED;
pub use hermes_substream::ACTION_PROPOSAL_SETTINGS_SELECTED as PROPOSAL_SETTINGS_SELECTED;
pub use hermes_substream::ACTION_PROPOSAL_UPDATED as PROPOSAL_UPDATED;
pub use hermes_substream::ACTION_PROPOSAL_VOTED as PROPOSAL_VOTED;
pub use hermes_substream::ACTION_SPACE_FAST_PATH_RESTRICTED as SPACE_FAST_PATH_RESTRICTED;
pub use hermes_substream::ACTION_SPACE_FAST_PATH_UNRESTRICTED as SPACE_FAST_PATH_UNRESTRICTED;
pub use hermes_substream::ACTION_SPACE_ID_ARCHIVED as SPACE_ID_ARCHIVED;
pub use hermes_substream::ACTION_SPACE_ID_CLEARED as SPACE_ID_CLEARED;
pub use hermes_substream::ACTION_SPACE_ID_MIGRATED as SPACE_MIGRATED;
pub use hermes_substream::ACTION_SPACE_ID_OVERRIDDEN as SPACE_OVERRIDDEN;
pub use hermes_substream::ACTION_SPACE_ID_RECOVERED as SPACE_ID_RECOVERED;
pub use hermes_substream::ACTION_SPACE_ID_REGISTERED as SPACE_REGISTERED;
pub use hermes_substream::ACTION_SPACE_LEFT as SPACE_LEFT;
pub use hermes_substream::ACTION_SPACE_TYPE_DECLARED as SPACE_TYPE_DECLARED;
pub use hermes_substream::ACTION_SUBSPACE_RELATED as SUBSPACE_RELATED;
pub use hermes_substream::ACTION_SUBSPACE_TOPIC_SET as SUBSPACE_TOPIC_SET;
pub use hermes_substream::ACTION_SUBSPACE_TOPIC_UNSET as SUBSPACE_TOPIC_UNSET;
pub use hermes_substream::ACTION_SUBSPACE_UNRELATED as SUBSPACE_UNRELATED;
pub use hermes_substream::ACTION_SUBSPACE_UNVERIFIED as SUBSPACE_UNVERIFIED;
pub use hermes_substream::ACTION_SUBSPACE_VERIFIED as SUBSPACE_VERIFIED;
pub use hermes_substream::ACTION_TOPIC_SET as TOPIC_SET;
pub use hermes_substream::ACTION_TOPIC_UNSET as TOPIC_UNSET;
pub use hermes_substream::ACTION_UNFLAGGED as UNFLAGGED;
pub use hermes_substream::ACTION_VOTING_SETTINGS_UPDATED as VOTING_SETTINGS_UPDATED;

// Legacy Governance V1 names, still sent by the web app (GEO-3803). Never match
// on these directly: `normalize_actions` / `canonical` rewrite them to their V2
// constant at decode time, so every filter downstream sees one hash.
pub use hermes_substream::ACTION_SUBSPACE_TOPIC_DECLARED as LEGACY_SUBSPACE_TOPIC_DECLARED;
pub use hermes_substream::ACTION_SUBSPACE_TOPIC_REMOVED as LEGACY_SUBSPACE_TOPIC_REMOVED;
pub use hermes_substream::ACTION_TOPIC_DECLARED as LEGACY_TOPIC_DECLARED;
pub use hermes_substream::ACTION_TOPIC_REMOVED as LEGACY_TOPIC_REMOVED;

// =============================================================================
// Permissionless Actions
// =============================================================================

// Curation kind (vote_kind 0) — the original triple.
pub use hermes_substream::ACTION_DOWNVOTED as DOWNVOTED;
pub use hermes_substream::ACTION_UNVOTED as UNVOTED;
pub use hermes_substream::ACTION_UPVOTED as UPVOTED;

// Stance kind (vote_kind 1) — "do you hold this position".
pub use hermes_substream::ACTION_AGREED as AGREED;
pub use hermes_substream::ACTION_DISAGREED as DISAGREED;
pub use hermes_substream::ACTION_UNAGREED as UNAGREED;

// Veracity kind (vote_kind 2) — "is this true".
pub use hermes_substream::ACTION_DISPUTED as DISPUTED;
pub use hermes_substream::ACTION_UNVERIFIED as UNVERIFIED;
pub use hermes_substream::ACTION_VERIFIED as VERIFIED;

// Interested kind (vote_kind 3) — positive and clear only; on a topic it is the
// topic follow.
pub use hermes_substream::ACTION_INTERESTED as INTERESTED;
pub use hermes_substream::ACTION_UNINTERESTED as UNINTERESTED;

// =============================================================================
// Space Type Constants
// =============================================================================

pub use hermes_substream::SPACE_TYPE_DAO;
pub use hermes_substream::SPACE_TYPE_EOA;

/// Check if an action matches a specific action type.
pub fn matches(action_bytes: &[u8], action_type: &[u8; 32]) -> bool {
    action_bytes == action_type
}

// =============================================================================
// Legacy aliases and action names (GEO-3803)
// =============================================================================

/// V1 → V2 aliases. Governance V2 (#759) renamed these four actions; the
/// registry accepts any hash and the web app still sends the V1 names, with the
/// same `topic`/`data` layout, so each V1 hash means exactly its V2 action.
pub const LEGACY_ALIASES: [(&[u8; 32], &[u8; 32], &str); 4] = [
    (&LEGACY_TOPIC_DECLARED, &TOPIC_SET, "TOPIC_DECLARED"),
    (&LEGACY_TOPIC_REMOVED, &TOPIC_UNSET, "TOPIC_REMOVED"),
    (
        &LEGACY_SUBSPACE_TOPIC_DECLARED,
        &SUBSPACE_TOPIC_SET,
        "SUBSPACE_TOPIC_DECLARED",
    ),
    (
        &LEGACY_SUBSPACE_TOPIC_REMOVED,
        &SUBSPACE_TOPIC_UNSET,
        "SUBSPACE_TOPIC_REMOVED",
    ),
];

/// The V2 constant a legacy V1 action hash stands for, or `None` when the hash
/// is not a legacy alias (V2 hashes and everything else map to `None`).
pub fn canonical(action_bytes: &[u8]) -> Option<&'static [u8; 32]> {
    LEGACY_ALIASES
        .iter()
        .find(|(legacy, _, _)| action_bytes == legacy.as_slice())
        .map(|(_, v2, _)| *v2)
}

/// Rewrite every legacy V1 action hash to its V2 constant, in place, and
/// return how many were rewritten. Call once, right after decoding a block's
/// `Actions`, so every filter downstream matches on the canonical hash only.
/// Order and every other field are untouched, so `sequence` and `is_last`
/// (both derived from positions) are unaffected.
pub fn normalize_actions(actions: &mut [crate::Action]) -> usize {
    let mut rewritten = 0;
    for action in actions.iter_mut() {
        if let Some(v2) = canonical(&action.action) {
            action.action = v2.to_vec();
            rewritten += 1;
        }
    }
    rewritten
}

#[cfg(test)]
mod tests {
    use alloy::primitives::keccak256;

    #[test]
    fn voting_settings_updated_matches_contract_hash() {
        assert_eq!(
            super::VOTING_SETTINGS_UPDATED,
            keccak256("GOVERNANCE.VOTING_SETTINGS_UPDATED").0,
        );
    }

    #[test]
    fn space_id_archived_matches_contract_hash() {
        assert_eq!(
            super::SPACE_ID_ARCHIVED,
            keccak256("GOVERNANCE.SPACE_ID_ARCHIVED").0,
        );
    }

    #[test]
    fn space_id_recovered_matches_contract_hash() {
        assert_eq!(
            super::SPACE_ID_RECOVERED,
            keccak256("GOVERNANCE.SPACE_ID_RECOVERED").0,
        );
    }

    #[test]
    fn space_id_cleared_matches_contract_hash() {
        assert_eq!(
            super::SPACE_ID_CLEARED,
            keccak256("GOVERNANCE.SPACE_ID_CLEARED").0,
        );
    }

    #[test]
    fn proposal_updated_matches_contract_hash() {
        assert_eq!(
            super::PROPOSAL_UPDATED,
            keccak256("GOVERNANCE.PROPOSAL_UPDATED").0,
        );
    }

    /// The eleven response actions, asserted against a live keccak.
    ///
    /// These constants are the shared source of truth for three parties that
    /// must agree byte for byte: the `setPermissionlessAction` registration
    /// calls, the SDK's calldata, and this indexer. A hash that does not match
    /// the registered one yields an action the registry never recognises — and
    /// on the indexer side, an event that is silently never decoded.
    #[test]
    fn response_actions_match_contract_hashes() {
        // Curation — pre-existing, included so a regression here is caught too.
        assert_eq!(super::UPVOTED, keccak256("PERMISSIONLESS.UPVOTED").0);
        assert_eq!(super::DOWNVOTED, keccak256("PERMISSIONLESS.DOWNVOTED").0);
        assert_eq!(super::UNVOTED, keccak256("PERMISSIONLESS.UNVOTED").0);

        // Stance.
        assert_eq!(super::AGREED, keccak256("PERMISSIONLESS.AGREED").0);
        assert_eq!(super::DISAGREED, keccak256("PERMISSIONLESS.DISAGREED").0);
        assert_eq!(super::UNAGREED, keccak256("PERMISSIONLESS.UNAGREED").0);

        // Veracity.
        assert_eq!(super::VERIFIED, keccak256("PERMISSIONLESS.VERIFIED").0);
        assert_eq!(super::DISPUTED, keccak256("PERMISSIONLESS.DISPUTED").0);
        assert_eq!(super::UNVERIFIED, keccak256("PERMISSIONLESS.UNVERIFIED").0);

        // Interested (GEO-3158). Pinned to literal bytes as well as a live
        // keccak, so the value Patrick registers on chain is in the repo.
        assert_eq!(super::INTERESTED, keccak256("PERMISSIONLESS.INTERESTED").0);
        assert_eq!(
            super::UNINTERESTED,
            keccak256("PERMISSIONLESS.UNINTERESTED").0
        );
        assert_eq!(
            alloy::primitives::hex::encode(super::INTERESTED),
            "18e7bf94be2682e4578e3ba940e4fcc2efcb9767062e812cfa4656d7d35377d9"
        );
        assert_eq!(
            alloy::primitives::hex::encode(super::UNINTERESTED),
            "7b8a4ef62d6765393827e9528a2d965c4ec8898864b8d0f6649c652c022b3864"
        );
    }

    /// All eleven hashes must be distinct, or one kind's events would be decoded
    /// as another's.
    #[test]
    fn response_action_hashes_are_pairwise_distinct() {
        let all = [
            super::UPVOTED,
            super::DOWNVOTED,
            super::UNVOTED,
            super::AGREED,
            super::DISAGREED,
            super::UNAGREED,
            super::VERIFIED,
            super::DISPUTED,
            super::UNVERIFIED,
            super::INTERESTED,
            super::UNINTERESTED,
        ];
        let unique: std::collections::HashSet<_> = all.iter().collect();
        assert_eq!(unique.len(), all.len(), "duplicate response action hash");
    }

    /// `PERMISSIONLESS.VERIFIED` and `GOVERNANCE.SUBSPACE_VERIFIED` are
    /// different concepts that share a word. Pin that they never share bytes.
    #[test]
    fn permissionless_verified_is_not_subspace_verified() {
        assert_ne!(super::VERIFIED, super::SUBSPACE_VERIFIED);
    }

    // ---- GEO-3803: legacy V1 aliases -------------------------------------

    #[test]
    fn legacy_aliases_match_their_v1_names() {
        assert_eq!(
            super::LEGACY_TOPIC_DECLARED,
            keccak256("GOVERNANCE.TOPIC_DECLARED").0
        );
        assert_eq!(
            super::LEGACY_TOPIC_REMOVED,
            keccak256("GOVERNANCE.TOPIC_REMOVED").0
        );
        assert_eq!(
            super::LEGACY_SUBSPACE_TOPIC_DECLARED,
            keccak256("GOVERNANCE.SUBSPACE_TOPIC_DECLARED").0
        );
        assert_eq!(
            super::LEGACY_SUBSPACE_TOPIC_REMOVED,
            keccak256("GOVERNANCE.SUBSPACE_TOPIC_REMOVED").0
        );
    }

    /// The exact values the web app sends: geogenesis
    /// apps/web/core/utils/contracts/space-registry.ts `GOVERNANCE_ACTIONS`.
    /// If the web constants change, this is the test to update.
    #[test]
    fn legacy_aliases_match_the_web_app_constants() {
        let hex = |b: [u8; 32]| alloy::primitives::hex::encode(b);
        assert_eq!(
            hex(super::LEGACY_TOPIC_DECLARED),
            "d020fbe5a0270daba9a9031c59402c03d5093ebfa8c4caa2203c2c72f95ce30c"
        );
        assert_eq!(
            hex(super::LEGACY_TOPIC_REMOVED),
            "6311f3c439cf906060b1d174cabab01645a7c8c628bee916f91d714030e3f195"
        );
        assert_eq!(
            hex(super::LEGACY_SUBSPACE_TOPIC_DECLARED),
            "f475121947612f07c138e5ac27aa31355aaea0da3096cea1b702daeb5e8477aa"
        );
        assert_eq!(
            hex(super::LEGACY_SUBSPACE_TOPIC_REMOVED),
            "98ee515a05d2eb17f8e4c1e997a36ee6a8eca03af78d98943787935f9e39adda"
        );
    }

    #[test]
    fn v2_topic_constants_match_their_names() {
        assert_eq!(super::TOPIC_SET, keccak256("GOVERNANCE.TOPIC_SET").0);
        assert_eq!(super::TOPIC_UNSET, keccak256("GOVERNANCE.TOPIC_UNSET").0);
        assert_eq!(
            super::SUBSPACE_TOPIC_SET,
            keccak256("GOVERNANCE.SUBSPACE_TOPIC_SET").0
        );
        assert_eq!(
            super::SUBSPACE_TOPIC_UNSET,
            keccak256("GOVERNANCE.SUBSPACE_TOPIC_UNSET").0
        );
    }

    #[test]
    fn canonical_maps_each_legacy_hash_to_its_v2_constant_only() {
        assert_eq!(
            super::canonical(&super::LEGACY_TOPIC_DECLARED),
            Some(&super::TOPIC_SET)
        );
        assert_eq!(
            super::canonical(&super::LEGACY_TOPIC_REMOVED),
            Some(&super::TOPIC_UNSET)
        );
        assert_eq!(
            super::canonical(&super::LEGACY_SUBSPACE_TOPIC_DECLARED),
            Some(&super::SUBSPACE_TOPIC_SET)
        );
        assert_eq!(
            super::canonical(&super::LEGACY_SUBSPACE_TOPIC_REMOVED),
            Some(&super::SUBSPACE_TOPIC_UNSET)
        );
        assert_eq!(super::canonical(&super::TOPIC_SET), None);
        assert_eq!(super::canonical(&super::EDITS_PUBLISHED), None);
        assert_eq!(super::canonical(&[0u8; 32]), None);
        assert_eq!(super::canonical(&[]), None);
    }

    fn action(hash: &[u8; 32], from: u8, to: u8) -> crate::Action {
        crate::Action {
            from_id: vec![from; 16],
            to_id: vec![to; 16],
            action: hash.to_vec(),
            topic: vec![9; 32],
            data: vec![1, 2, 3],
        }
    }

    #[test]
    fn normalize_actions_rewrites_only_legacy_hashes_and_keeps_order() {
        let unknown = [0xab; 32];
        let mut actions = vec![
            action(&super::EDITS_PUBLISHED, 1, 1),
            action(&super::LEGACY_TOPIC_DECLARED, 1, 1),
            action(&unknown, 2, 3),
            action(&super::TOPIC_SET, 4, 4),
            action(&super::LEGACY_SUBSPACE_TOPIC_DECLARED, 5, 6),
        ];
        let before = actions.clone();

        assert_eq!(super::normalize_actions(&mut actions), 2);

        assert_eq!(actions[0], before[0]);
        assert_eq!(actions[1].action, super::TOPIC_SET.to_vec());
        assert_eq!(actions[2], before[2]);
        assert_eq!(actions[3], before[3]);
        assert_eq!(actions[4].action, super::SUBSPACE_TOPIC_SET.to_vec());
        // Only the hash changes.
        for (a, b) in actions.iter().zip(&before) {
            assert_eq!(
                (&a.from_id, &a.to_id, &a.topic, &a.data),
                (&b.from_id, &b.to_id, &b.topic, &b.data)
            );
        }
    }

    #[test]
    fn normalize_actions_on_an_all_v2_block_is_a_no_op() {
        let mut actions = vec![
            action(&super::TOPIC_SET, 1, 1),
            action(&super::MEMBER_ADDED, 1, 2),
        ];
        let before = actions.clone();
        assert_eq!(super::normalize_actions(&mut actions), 0);
        assert_eq!(actions, before);
    }
}
