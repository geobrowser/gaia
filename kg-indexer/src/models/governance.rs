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
    None,
    Yes,
    No,
    Abstain,
}

/// Decoded payload for a proposal action
#[derive(Clone, Debug, PartialEq)]
pub enum ProposalActionPayload {
    /// Add a member to the space
    AddMember { target_address: String },
    /// Remove a member from the space
    RemoveMember { target_address: String },
    /// Add an editor to the space
    AddEditor { target_address: String },
    /// Remove an editor from the space
    RemoveEditor { target_address: String },
    /// Unflag an editor
    UnflagEditor { target_address: String },
    /// Publish content
    Publish { content_uri: Vec<u8>, metadata: Vec<u8> },
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
#[derive(Clone, Debug)]
pub struct ProposalItem {
    pub id: Uuid,
    pub space_id: Uuid,
    pub proposer_id: Uuid,
    pub voting_mode: VotingMode,
    pub start_date: i64,
    pub end_date: i64,
    pub quorum: i64,
    pub threshold: i64,
    pub executed_at: Option<i64>,
    pub created_at: i64,
    pub created_at_block: i64,
}

/// An action within a proposal
#[derive(Clone, Debug)]
pub struct ProposalActionItem {
    pub proposal_id: Uuid,
    pub index: i32,
    pub to_address: String,
    pub value: String,
    pub data: Vec<u8>,
    pub payload: ProposalActionPayload,
}

/// A vote on a proposal
#[derive(Clone, Debug)]
pub struct ProposalVoteItem {
    pub proposal_id: Uuid,
    pub voter_id: Uuid,
    pub space_id: Uuid,
    pub vote: VoteOption,
    pub created_at: i64,
    pub created_at_block: i64,
}
