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
