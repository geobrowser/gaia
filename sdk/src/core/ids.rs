// Property IDs
pub const NAME_PROPERTY_ID: &str = "a126ca53-0c8e-48d5-b888-82c734c38935";
pub const DESCRIPTION_PROPERTY_ID: &str = "9b1f76ff-9711-404c-861e-59dc3fa7d037";
pub const IMAGE_URL_PROPERTY_ID: &str = "8a743832-c094-4a62-b665-0c3cc2f9c7bc";

/// Deprecated: This is actually a relation type ID, not a property ID.
/// Use `AVATAR_RELATION_TYPE_ID` instead.
pub const AVATAR_PROPERTY_ID: &str = "1155beff-fad5-49b7-a2e0-da4777b8792c";

// Relation Type IDs
/// The relation type ID for "type" relations.
/// When a relation has this type ID, it indicates that the `from_entity` has a type of `to_entity`.
pub const TYPE_RELATION_TYPE_ID: &str = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1";

/// The relation type ID for avatar relations.
/// When a relation has this type ID, it indicates that the `from_entity` has an avatar of `to_entity`.
/// The `to_entity` is an image entity whose `IMAGE_URL_PROPERTY_ID` value contains the actual URL.
pub const AVATAR_RELATION_TYPE_ID: &str = "1155beff-fad5-49b7-a2e0-da4777b8792c";

/// The relation type ID for cover image relations.
/// When a relation has this type ID, it indicates that the `from_entity` has a cover image of `to_entity`.
/// The `to_entity` is an image entity whose `IMAGE_URL_PROPERTY_ID` value contains the actual URL.
pub const COVER_RELATION_TYPE_ID: &str = "34f53507-2e6b-42c5-a844-43981a77cfa2";

// System namespace — derived from Uuid::NAMESPACE_URL + "geo:system"
pub const GEO_SYSTEM_NAMESPACE: &str = "ae1e004d-b125-57d8-b3bf-e85a2484f129";

// System type entity IDs
pub const SYSTEM_TYPE_ID: &str = "2ff7ea09-8b9e-50bc-9be7-8a0cafa268d0";
pub const SPACE_TYPE_ID: &str = "f4ce7263-ed14-56c5-aafe-8b74428ce812";
pub const PROPOSAL_TYPE_ID: &str = "1cb9d5ba-c730-52a7-bc73-1f2c3ff5a330";
pub const EOA_SPACE_TYPE_ID: &str = "09d28123-178f-5828-b85a-81979389c746";
pub const DAO_SPACE_TYPE_ID: &str = "afd76215-db11-5cba-81b9-e7f77f865805";

// Protected property IDs (system-managed, user edits targeting these are dropped)
pub const SPACE_ADDRESS_PROPERTY_ID: &str = "8f65b58c-d001-5bac-b1d3-3a66ae23193c";
pub const VOTING_MODE_PROPERTY_ID: &str = "af26ed41-de0f-5b4f-a093-e9d3f683db07";
pub const SPACE_ID_PROPERTY_ID: &str = "712adc1f-e950-5d14-bbd8-bf2166fe59c1";
pub const CREATED_AT_BLOCK_PROPERTY_ID: &str = "da2952fb-17d6-53f3-b521-52e254106e0b";
pub const SCORE_PROPERTY_ID: &str = "85a4668a-42fa-4f48-8969-c0a9de0c294b";

// Rank submission IDs. Canonical values mirrored from the grc-20 SDK
// (`@graphprotocol/grc-20` core/ids/system) — these are the user-authored
// shapes the ranking-indexer detects on the `knowledge.edits` stream.
//
// The indexer-owned output IDs (`RANK_POSITION` etc.) are defined below: they
// are system-minted (not SDK shapes), assigned as a random v4 like
// `SCORE_PROPERTY_ID` rather than derived.
pub const RANK_TYPE_ID: &str = "5c74731d-fabb-4dc8-b5c5-3346521c639a";
pub const RANK_TYPE_PROPERTY_ID: &str = "48e01bc8-324e-48c2-a6c9-cab3f49290c6";
pub const RANK_VOTES_RELATION_TYPE_ID: &str = "19a4cfff-45f2-4150-abf2-af0f43eb2eec";
pub const VOTE_ORDINAL_VALUE_PROPERTY_ID: &str = "49ee1b89-1820-4e75-a1ae-38a2dcaad4a5";
pub const VOTE_WEIGHTED_VALUE_PROPERTY_ID: &str = "103701dd-cabe-4a8e-835b-10345327b647";

// Ranking-block IDs. PROVISIONAL — sourced from geo-sdk#89
// (feat/ranks-create-update), which is not yet merged; pin to the released
// values once it lands. These are the user-authored ranking-block shapes the
// indexer detects (block type, rank→block link, submission window, and the
// aggregation restriction with its default "Members and editors" value).
pub const RANKING_BLOCK_TYPE_ID: &str = "150db6de-fe23-44f0-805a-fa57502e2c32";
pub const RANK_BLOCK_RELATION_TYPE_ID: &str = "09c219c1-03d1-4d2a-a5c7-8edbf2d0182a";
pub const RANK_START_DATE_PROPERTY_ID: &str = "eed03a04-0acd-4a9e-81e0-8272ed70a817";
pub const RANK_END_DATE_PROPERTY_ID: &str = "b08b8f63-dc1e-4156-8b08-19946f2b011c";
pub const RANK_AGGREGATION_RESTRICTION_PROPERTY_ID: &str = "1e4caa2d-e331-4efa-8ac2-4e8d9d3e9fe9";
pub const RANK_RESTRICTION_MEMBERS_AND_EDITORS_ID: &str = "10a7b103-90f9-4a72-8087-935052ffaa69";
pub const RANK_FILTER_PROPERTY_ID: &str = "14a46854-bfd1-4b18-8215-2785c2dab9f3";

// Indexer-owned ranking output IDs. System-minted (not SDK shapes), assigned as
// a random v4 like SCORE_PROPERTY_ID. `RANK_POSITION` is the relation type the
// ranking-indexer publishes per ranked target — no user edit may author it, so
// it is reserved in `PROTECTED_RELATION_TYPE_IDS` below.
pub const RANK_POSITION_RELATION_TYPE_ID: &str = "890deffb-3843-49fa-8269-74000e3dcef6";
// `Aggregated rankings` is an indexer-owned relation type: from a published
// RANK_POSITION result back to the individual Rank submissions that were
// aggregated to produce its ordering (provenance). Reserved below so user edits
// can't forge it.
pub const AGGREGATED_RANKINGS_RELATION_TYPE_ID: &str = "67651da6-11a9-4246-9ff4-039aafbe9e43";
// `Rank position value` is the indexer-owned integer property carrying an
// entity's aggregated score within a ranking block (surfaced on the published
// RANK_POSITION relation's reified entity). Reserved in PROTECTED_PROPERTY_IDS.
pub const RANK_POSITION_VALUE_PROPERTY_ID: &str = "e1ffac9f-beb4-4654-8a50-00e1d6c662fb";

/// Onchain-derived ID of the "root" space whose editors are authorized to
/// publish edits that mutate the system property-definition entities — i.e.
/// relations whose `from` or `to` is in `PROTECTED_PROPERTY_IDS`. Edits from
/// any other space targeting those entities as relation endpoints are
/// silently dropped by the kg-indexer.
///
/// NOTE: Space IDs are per-deployment (`keccak('grc20.space', account, nonce,
/// chainId)`), so this constant pins the SDK to a single chain/env. Update
/// it if the root space is migrated, or fork the SDK if you ever run
/// multiple envs from one binary.
pub const ROOT_SPACE_ID: &str = "a19c345a-b986-6679-b001-d7d2138d88a1";

// Protected relation type IDs
pub const SYSTEM_TYPES_RELATION_TYPE_ID: &str = "88b3d6ad-288c-529c-a212-0e1c24819185";

// Collected sets for protection checks
pub const PROTECTED_PROPERTY_IDS: &[&str] = &[
    SPACE_ADDRESS_PROPERTY_ID,
    VOTING_MODE_PROPERTY_ID,
    SPACE_ID_PROPERTY_ID,
    CREATED_AT_BLOCK_PROPERTY_ID,
    SCORE_PROPERTY_ID,
    RANK_POSITION_VALUE_PROPERTY_ID,
];

pub const PROTECTED_RELATION_TYPE_IDS: &[&str] = &[
    SYSTEM_TYPES_RELATION_TYPE_ID,
    RANK_POSITION_RELATION_TYPE_ID,
    AGGREGATED_RANKINGS_RELATION_TYPE_ID,
];

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    /// Re-derive every system UUID from its documented input and verify
    /// it matches the hardcoded constant. This prevents accidental edits
    /// and ensures the derivation inputs in the design doc stay correct.
    #[test]
    fn system_ids_match_derivations() {
        let namespace_url = Uuid::parse_str("6ba7b811-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let ns = Uuid::new_v5(&namespace_url, b"geo:system");
        assert_eq!(GEO_SYSTEM_NAMESPACE, ns.to_string());

        let cases: &[(&str, &str)] = &[
            (SYSTEM_TYPE_ID, "type:System"),
            (SPACE_TYPE_ID, "type:Space"),
            (PROPOSAL_TYPE_ID, "type:Proposal"),
            (SPACE_ADDRESS_PROPERTY_ID, "property:SpaceAddress"),
            (VOTING_MODE_PROPERTY_ID, "property:VotingMode"),
            (SPACE_ID_PROPERTY_ID, "property:SpaceId"),
            (CREATED_AT_BLOCK_PROPERTY_ID, "property:CreatedAtBlock"),
            (SYSTEM_TYPES_RELATION_TYPE_ID, "relation_type:SystemTypes"),
            (EOA_SPACE_TYPE_ID, "type:EoaSpace"),
            (DAO_SPACE_TYPE_ID, "type:DaoSpace"),
        ];

        for (constant, input) in cases {
            let derived = Uuid::new_v5(&ns, input.as_bytes());
            assert_eq!(
                *constant,
                derived.to_string(),
                "Mismatch for derivation input '{}'",
                input
            );
        }
    }

    #[test]
    fn eoa_and_dao_space_type_ids_are_unique() {
        let all_ids = [
            SYSTEM_TYPE_ID,
            SPACE_TYPE_ID,
            PROPOSAL_TYPE_ID,
            EOA_SPACE_TYPE_ID,
            DAO_SPACE_TYPE_ID,
        ];
        for i in 0..all_ids.len() {
            for j in (i + 1)..all_ids.len() {
                assert_ne!(
                    all_ids[i], all_ids[j],
                    "Collision between index {} and {}",
                    i, j
                );
            }
        }
    }

    #[test]
    fn protected_property_ids_contains_all_system_properties() {
        let expected = [
            SPACE_ADDRESS_PROPERTY_ID,
            VOTING_MODE_PROPERTY_ID,
            SPACE_ID_PROPERTY_ID,
            CREATED_AT_BLOCK_PROPERTY_ID,
            SCORE_PROPERTY_ID,
            RANK_POSITION_VALUE_PROPERTY_ID,
        ];
        assert_eq!(PROTECTED_PROPERTY_IDS.len(), expected.len());
        for id in &expected {
            assert!(
                PROTECTED_PROPERTY_IDS.contains(id),
                "{} missing from PROTECTED_PROPERTY_IDS",
                id
            );
        }
    }

    #[test]
    fn protected_relation_type_ids_contains_system_types() {
        assert_eq!(PROTECTED_RELATION_TYPE_IDS.len(), 3);
        assert!(PROTECTED_RELATION_TYPE_IDS.contains(&SYSTEM_TYPES_RELATION_TYPE_ID));
        assert!(PROTECTED_RELATION_TYPE_IDS.contains(&RANK_POSITION_RELATION_TYPE_ID));
        assert!(PROTECTED_RELATION_TYPE_IDS.contains(&AGGREGATED_RANKINGS_RELATION_TYPE_ID));
    }
}
