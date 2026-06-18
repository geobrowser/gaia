use uuid::Uuid;

/// Voting mode for a proposal
#[derive(Clone, Debug, PartialEq)]
pub enum VotingMode {
    Fast,
    Slow,
}

/// Vote option for governance proposals
#[derive(Clone, Debug, PartialEq)]
pub enum VoteOption {
    Yes,
    No,
    Abstain,
}

/// Decoded payload for a proposal action
#[derive(Clone, Debug, PartialEq)]
pub enum ProposalActionPayload {
    /// Add a member to the space
    AddMember { target_id: Uuid },
    /// Remove a member from the space
    RemoveMember { target_id: Uuid },
    /// Add an editor to the space
    AddEditor { target_id: Uuid },
    /// Remove an editor from the space
    RemoveEditor { target_id: Uuid },
    /// Unflag an editor (unrestrict fast path)
    UnflagEditor { target_id: Uuid },
    /// Publish content
    Publish {
        content_uri: String,
        metadata: Vec<u8>,
    },
    /// Flag content
    Flag { content_id: Vec<u8> },
    /// Unflag content
    Unflag { content_id: Vec<u8> },
    /// Update voting settings (V2 — 7 fields matching contract `VotingSettings`)
    UpdateVotingSettings {
        partial_percentage_support_threshold: u64,
        universal_percentage_support_threshold: u64,
        flat_support_threshold: u64,
        quorum: u64,
        duration: u64,
        disable_fast_path_access_for_new_members: bool,
        execution_grace_period: u64,
    },
    /// Add verified subspace edge
    SubspaceVerified { target_space_id: Uuid },
    /// Remove verified subspace edge
    SubspaceUnverified { target_space_id: Uuid },
    /// Add related subspace edge
    SubspaceRelated { target_space_id: Uuid },
    /// Remove related subspace edge
    SubspaceUnrelated { target_space_id: Uuid },
    /// Declare a topic on a subspace
    SubspaceTopicDeclared { target_topic_id: Uuid },
    /// Remove a topic from a subspace
    SubspaceTopicRemoved { target_topic_id: Uuid },
    /// Set the space topic
    SetTopic { target_topic_id: Uuid },
    /// Unset the current space topic
    UnsetTopic { target_topic_id: Uuid },
    /// Unknown or undecoded action
    Unknown,
}

/// Immutable identity of a governance proposal.
///
/// Stored once on CREATE. Per-version mutable state (voting settings, tally
/// counts, name) lives in [`ProposalVersionItem`] rows scoped by
/// `(proposal_id, proposal_version)`. Identity-level fields — `current_version`
/// (pointer to the active version row) and `executed_at` (stamped once on
/// execution) — live on the `proposals` row itself.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ProposalIdentity {
    pub id: Uuid,
    pub space_id: Uuid,
    pub proposed_by: Uuid,
    pub created_at: i64,
    pub created_at_block: i64,
}

/// Per-version proposal state. Appended on every CREATE (version 1) and UPDATE
/// (next version). The `proposal_version` number is assigned by the storage
/// layer — the handler produces this item version-agnostically.
///
/// Settings escalation (fast→slow on a NO vote) UPDATES the current version's
/// row in place rather than appending — the contract does not bump the version
/// for escalation.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ProposalVersionItem {
    pub voting_mode: VotingMode,
    pub start_time: i64,
    pub end_time: i64,
    pub quorum: i64,
    /// Legacy threshold: populated with the voting-mode-dependent value
    /// (`flat_support_threshold` for Fast, `partial_percentage_support_threshold`
    /// for Slow) for backward compatibility with the existing DB column.
    /// New code should prefer the individual V2 fields below.
    pub threshold: i64,
    /// V2: slow-path late execution threshold (0..RATIO_BASE).
    pub partial_percentage_support_threshold: i64,
    /// V2: slow-path early execution threshold (0..RATIO_BASE).
    pub universal_percentage_support_threshold: i64,
    /// V2: fast-path absolute YES votes needed.
    pub flat_support_threshold: i64,
    /// V2: inclusive upper bound timestamp for execution. `None` for V1 rows.
    pub execute_by: Option<i64>,
    /// Human-readable name derived from this version's actions.
    pub name: Option<String>,
    /// Timestamp when THIS version was created (may be the proposal's original
    /// creation time for v1, or an update block's time for later versions).
    pub version_created_at: i64,
    pub version_created_at_block: i64,
}

/// An action within a specific proposal version. PK is
/// `(proposal_id, proposal_version, index)` — no standalone uuid column.
///
/// `index` is the 0-based position of the action in the proposal's action
/// array. `proposal_version` is set by the caller (the storage layer knows
/// which version the accompanying version row received on insert).
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ProposalActionItem {
    pub proposal_id: Uuid,
    pub proposal_version: i32,
    pub index: i32,
    pub payload: ProposalActionPayload,
}

/// A vote on a proposal
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ProposalVoteItem {
    pub proposal_id: Uuid,
    pub voter_id: Uuid,
    pub space_id: Uuid,
    pub vote: VoteOption,
    pub created_at: i64,
    pub created_at_block: i64,
    /// V2: the proposal version being voted on. `uint8` on-chain, widened
    /// to `i32` for Postgres. Used by storage to reject stale votes and
    /// to scope vote aggregation per-version.
    pub proposal_version: i32,
}

/// DAO-global voting settings for a space (V2).
///
/// Upserted on every `VOTING_SETTINGS_UPDATED` action event.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct SpaceVotingSettingsItem {
    pub space_id: Uuid,
    pub partial_percentage_support_threshold: i64,
    pub universal_percentage_support_threshold: i64,
    pub flat_support_threshold: i64,
    pub quorum: i64,
    pub duration: i64,
    pub disable_fast_path_access_for_new_members: bool,
    pub execution_grace_period: i64,
    pub updated_at: i64,
    pub updated_at_block: i64,
}
