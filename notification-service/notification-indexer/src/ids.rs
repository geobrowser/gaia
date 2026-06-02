//! `Uuid` helpers over the canonical knowledge-graph IDs.
//!
//! The ID *values* live in the `sdk` crate (`sdk::core::ids` for system IDs,
//! `sdk::core::content_ids` for content-type IDs). This module just parses the
//! ones the indexer needs into `Uuid`s (these are protocol constants, so a
//! parse failure is a programmer error and panics).

use sdk::core::{content_ids, ids as sdk_ids};
use uuid::Uuid;

macro_rules! id_fn {
    ($fn_name:ident, $const:expr, $what:literal) => {
        #[doc = concat!("The ", $what, " ID as a `Uuid`.")]
        pub fn $fn_name() -> Uuid {
            Uuid::parse_str($const).expect(concat!("valid ", $what, " ID"))
        }
    };
}

id_fn!(
    types_relation_type,
    sdk_ids::TYPE_RELATION_TYPE_ID,
    "Types relation"
);
id_fn!(name_property, sdk_ids::NAME_PROPERTY_ID, "Name property");
id_fn!(space_type, content_ids::SPACE_TYPE_ID, "Space type");
id_fn!(bounty_type, content_ids::BOUNTY_TYPE_ID, "Bounty type");
id_fn!(comment_type, content_ids::COMMENT_TYPE_ID, "Comment type");
id_fn!(
    reply_to_property,
    content_ids::REPLY_TO_PROPERTY_ID,
    "Reply-to property"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_ids_parse() {
        // Each helper panics on an invalid const, so calling them all validates
        // every ID sourced from the sdk crate is a well-formed UUID.
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
