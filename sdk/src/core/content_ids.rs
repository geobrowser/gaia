//! Well-known content type entity IDs.
//!
//! These are entity IDs for common content types in the knowledge graph.
//! Unlike system IDs (which are deterministically derived via UUIDv5),
//! these are created as regular entities and their IDs are fixed by convention.

/// Comment type entity. Used for user comments on entities.
pub const COMMENT_TYPE_ID: &str = "82f6123a-0323-4c6c-a811-701c5bc026e9";

/// Reply-to property — the relation type linking a comment to the entity it
/// replies to (comment → parent). Used to model comment threads.
pub const REPLY_TO_PROPERTY_ID: &str = "310d4a24-0e5b-451c-b215-1bfce40d0fe6";

/// Bounty type entity. Used to identify bounty entities via Types relations.
pub const BOUNTY_TYPE_ID: &str = "808af0ba-d588-4e33-91f0-9dd4b25e18be";

/// Space type entity (content convention; distinct from the UUIDv5-derived
/// `ids::SPACE_TYPE_ID`). Used to identify a space's "front page entity" via a
/// Types relation (entity → Space type).
pub const SPACE_TYPE_ID: &str = "362c1dbd-dc64-44bb-a3c4-652f38a642d7";
