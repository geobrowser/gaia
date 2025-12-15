//! Hermes-specific configuration for connecting to hermes-substream.
//!
//! This module provides the module names and package file paths that transformers
//! use to subscribe to specific event types from hermes-substream.

/// Path to the hermes-substream package file
pub const HERMES_SPKG: &str = "hermes-substream.spkg";

/// Available output modules in hermes-substream.
///
/// Each transformer subscribes to a specific module based on the events it needs.
/// Use `module.as_str()` to get the module name for the substreams API.
///
/// ## Single vs Multiple Event Types
///
/// The substreams protocol only supports consuming a single output module per stream
/// in production mode. If your transformer needs events from multiple modules, use
/// [`HermesModule::Actions`] to receive all raw actions and filter client-side.
///
/// See `docs/decisions/0001-multiple-substreams-modules-consumers.md` for more details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HermesModule {
    /// Raw actions - use this when you need multiple event types.
    /// Filter client-side based on the action type.
    Actions,

    // Governance events - space lifecycle
    SpacesRegistered,
    SpacesMigrated,

    // Governance events - proposals
    ProposalsCreated,
    ProposalsVoted,
    ProposalsExecuted,

    // Governance events - membership
    EditorsAdded,
    EditorsRemoved,
    MembersAdded,
    MembersRemoved,
    EditorsFlagged,
    EditorsUnflagged,
    SpacesLeft,

    // Governance events - topics
    TopicsDeclared,

    // Knowledge graph events
    EditsPublished,
    ContentFlagged,

    // Subspace events
    SubspacesAdded,
    SubspacesRemoved,

    // Permissionless events - voting
    ObjectsUpvoted,
    ObjectsDownvoted,
    ObjectsUnvoted,
}

impl HermesModule {
    /// Returns the module name as expected by the substreams API.
    pub fn as_str(&self) -> &'static str {
        match self {
            HermesModule::Actions => "map_actions",

            HermesModule::SpacesRegistered => "map_spaces_registered",
            HermesModule::SpacesMigrated => "map_spaces_migrated",

            HermesModule::ProposalsCreated => "map_proposals_created",
            HermesModule::ProposalsVoted => "map_proposals_voted",
            HermesModule::ProposalsExecuted => "map_proposals_executed",

            HermesModule::EditorsAdded => "map_editors_added",
            HermesModule::EditorsRemoved => "map_editors_removed",
            HermesModule::MembersAdded => "map_members_added",
            HermesModule::MembersRemoved => "map_members_removed",
            HermesModule::EditorsFlagged => "map_editors_flagged",
            HermesModule::EditorsUnflagged => "map_editors_unflagged",
            HermesModule::SpacesLeft => "map_spaces_left",

            HermesModule::TopicsDeclared => "map_topics_declared",

            HermesModule::EditsPublished => "map_edits_published",
            HermesModule::ContentFlagged => "map_content_flagged",

            HermesModule::SubspacesAdded => "map_subspaces_added",
            HermesModule::SubspacesRemoved => "map_subspaces_removed",

            HermesModule::ObjectsUpvoted => "map_objects_upvoted",
            HermesModule::ObjectsDownvoted => "map_objects_downvoted",
            HermesModule::ObjectsUnvoted => "map_objects_unvoted",
        }
    }
}

impl std::fmt::Display for HermesModule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
