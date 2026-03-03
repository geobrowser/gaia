use uuid::Uuid;

/// Type of explicit subspace relationship (space → space).
/// Must match the Postgres `"subspaceType"` enum values.
#[derive(Clone, Debug)]
pub enum SubspaceType {
    Verified,
    Related,
}

impl SubspaceType {
    /// SQL-compatible string value for the Postgres enum.
    pub fn as_str(&self) -> &'static str {
        match self {
            SubspaceType::Verified => "verified",
            SubspaceType::Related => "related",
        }
    }
}

/// An explicit subspace edge (verified or related): space → space.
#[derive(Clone, Debug)]
pub struct SubspaceItem {
    pub subspace_id: Uuid,
    pub parent_space_id: Uuid,
    pub subspace_type: SubspaceType,
}

/// A topic subspace edge: space → topic.
#[derive(Clone, Debug)]
pub struct SubspaceTopicItem {
    pub space_id: Uuid,
    pub topic_id: Uuid,
}

/// Result of handling a trust extension event. Distinguishes all four
/// storage operations so the dispatch site can route to the correct
/// storage function.
#[derive(Clone, Debug)]
pub enum SubspaceChange {
    /// Insert an explicit edge (verified/related)
    InsertExplicit(SubspaceItem),
    /// Remove an explicit edge (verified/related)
    RemoveExplicit(SubspaceItem),
    /// Insert a topic edge
    InsertTopic(SubspaceTopicItem),
    /// Remove a topic edge
    RemoveTopic(SubspaceTopicItem),
}
