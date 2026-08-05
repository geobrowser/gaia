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

// =============================================================================
// Space Type Constants
// =============================================================================

pub use hermes_substream::SPACE_TYPE_DAO;
pub use hermes_substream::SPACE_TYPE_EOA;

/// Check if an action matches a specific action type.
pub fn matches(action_bytes: &[u8], action_type: &[u8; 32]) -> bool {
    action_bytes == action_type
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

    /// The nine response actions, asserted against a live keccak.
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
    }

    /// All nine hashes must be distinct, or one kind's events would be decoded
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
}
