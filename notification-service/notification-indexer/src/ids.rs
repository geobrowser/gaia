//! Well-known knowledge-graph system IDs.
//!
//! Mirrored from the geo-sdk (`src/core/ids/{system,content}.ts`); the `grc-20`
//! Rust crate does not yet export these as constants. Once it does, prefer
//! importing from there. Stored as string consts with helpers that parse to a
//! `Uuid` (these are protocol constants, so a parse failure is a programmer
//! error and panics).

use uuid::Uuid;

/// "Types" relation type — entity → type membership (e.g. entity → Bounty type).
pub const TYPES_RELATION_TYPE_ID: &str = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1";
/// "Space" type entity — identifies space entities via Types relations.
pub const SPACE_TYPE_ID: &str = "362c1dbd-dc64-44bb-a3c4-652f38a642d7";
/// "Bounty" type entity.
pub const BOUNTY_TYPE_ID: &str = "808af0ba-d588-4e33-91f0-9dd4b25e18be";
/// "Comment" type entity.
pub const COMMENT_TYPE_ID: &str = "82f6123a-0323-4c6c-a811-701c5bc026e9";
/// "Reply to" property — the relation type linking a comment to the entity it
/// replies to (comment → parent).
pub const REPLY_TO_PROPERTY_ID: &str = "310d4a24-0e5b-451c-b215-1bfce40d0fe6";
/// "Name" property — entity display name.
pub const NAME_PROPERTY_ID: &str = "a126ca53-0c8e-48d5-b888-82c734c38935";

macro_rules! id_fn {
    ($fn_name:ident, $const_name:ident, $what:literal) => {
        #[doc = concat!("Parse the ", $what, " system ID into a `Uuid`.")]
        pub fn $fn_name() -> Uuid {
            Uuid::parse_str($const_name).expect(concat!("valid ", $what, " system ID"))
        }
    };
}

id_fn!(
    types_relation_type,
    TYPES_RELATION_TYPE_ID,
    "Types relation"
);
id_fn!(space_type, SPACE_TYPE_ID, "Space type");
id_fn!(bounty_type, BOUNTY_TYPE_ID, "Bounty type");
id_fn!(comment_type, COMMENT_TYPE_ID, "Comment type");
id_fn!(reply_to_property, REPLY_TO_PROPERTY_ID, "Reply-to property");
id_fn!(name_property, NAME_PROPERTY_ID, "Name property");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_ids_parse() {
        // Each helper panics on an invalid const, so calling them all validates
        // every system ID compiles to a well-formed UUID.
        let _ = (
            types_relation_type(),
            space_type(),
            bounty_type(),
            comment_type(),
            reply_to_property(),
            name_property(),
        );
    }
}
