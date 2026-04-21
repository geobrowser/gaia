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
    UnsetTopic,
    /// Unknown or undecoded action
    Unknown,
}

/// A governance proposal
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ProposalItem {
    pub id: Uuid,
    pub space_id: Uuid,
    pub proposed_by: Uuid,
    pub voting_mode: VotingMode,
    pub start_time: i64,
    pub end_time: i64,
    pub quorum: i64,
    /// Legacy threshold: populated with the voting-mode-dependent value
    /// (`flat_support_threshold` for Fast, `partial_percentage_support_threshold`
    /// for Slow) for backward compatibility with the existing DB column.
    /// New code should prefer the individual V2 fields below.
    pub threshold: i64,
    pub executed_at: Option<i64>,
    pub created_at: i64,
    pub created_at_block: i64,
    /// Human-readable name derived from proposal actions
    pub name: Option<String>,
    /// V2: monotonically-incrementing proposal version (starts at 1 on create,
    /// bumped on update). `uint8` on-chain, widened to `i32` for Postgres.
    pub proposal_version: i32,
    /// V2: slow-path late execution threshold (0..RATIO_BASE).
    pub partial_percentage_support_threshold: i64,
    /// V2: slow-path early execution threshold (0..RATIO_BASE).
    pub universal_percentage_support_threshold: i64,
    /// V2: fast-path absolute YES votes needed.
    pub flat_support_threshold: i64,
    /// V2: inclusive upper bound timestamp for execution. `None` for V1 rows.
    pub execute_by: Option<i64>,
}

/// An action within a proposal.
/// ID is deterministic (derived from proposal_id + index).
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ProposalActionItem {
    pub id: Uuid,
    pub proposal_id: Uuid,
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
/// Upserted on every `VOTING_SETTINGS_UPDATED` action event. `total_editors`
/// is a denormalized counter maintained by the KG indexer consumer
/// (`EDITOR_ADDED` / `EDITOR_REMOVED` events — see GEO-482); the handler
/// returns 0 for it and storage defaults to 0 on insert so the counter is
/// preserved across settings updates.
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
    pub total_editors: i64,
    pub updated_at: i64,
    pub updated_at_block: i64,
}
