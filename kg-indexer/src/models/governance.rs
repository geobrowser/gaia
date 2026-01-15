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
    /// Update voting settings
    UpdateVotingSettings {
        quorum: u64,
        fast_threshold: u64,
        slow_threshold: u64,
        duration: u64,
    },
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
    pub threshold: i64,
    pub executed_at: Option<i64>,
    pub created_at: i64,
    pub created_at_block: i64,
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
}
