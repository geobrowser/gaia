//! OpenSearch index configuration and mappings.
//!
//! This module defines the index settings and mappings for the entity search index.

use serde_json::{json, Value};

/// Configuration for the search index.
#[derive(Debug, Clone)]
pub struct IndexConfig {
    /// The alias name for the search index (used for all operations).
    pub alias: String,
    /// The version number for the index (e.g., 0 for "entities_v0").
    pub version: u32,
}

impl IndexConfig {
    /// Create a new index configuration.
    ///
    /// # Arguments
    ///
    /// * `alias` - The index alias name
    /// * `version` - The version number
    pub fn new(alias: impl Into<String>, version: u32) -> Self {
        Self {
            alias: alias.into(),
            version,
        }
    }
}

/// The base name of the search index (without version).
pub const INDEX_NAME: &str = "entities";

/// Get the versioned index name.
///
/// # Arguments
///
/// * `version` - The version number (defaults to 0 if None)
///
/// # Returns
///
/// The versioned index name (e.g., "entities_v0")
pub fn get_versioned_index_name(version: Option<u32>) -> String {
    let v = version.unwrap_or(0);
    format!("{}_v{}", INDEX_NAME, v)
}

/// Get the index settings and mappings for the entity search index.
///
/// The configuration includes:
/// - **search_as_you_type**: Built-in field type for autocomplete on name and description
/// - **rank_feature**: Score fields optimized for relevance boosting
/// - **Keyword fields**: For filtering and exact ID lookups
///
/// # Sharding Configuration
///
/// - 1 primary shard
/// - 1 replica for redundancy
///
/// # Arguments
///
/// * `version` - Optional version number (currently unused, reserved for future version-specific settings)
pub fn get_index_settings(_version: Option<u32>) -> Value {
    json!({
        "settings": {
            "number_of_shards": 1,
            "number_of_replicas": 1
        },
        "mappings": {
            "properties": {
                "entity_id": {
                    "type": "keyword"
                },
                "space_id": {
                    "type": "keyword"
                },
                "name": {
                    "type": "search_as_you_type",
                    "fields": {
                        "raw": {
                            "type": "keyword"
                        }
                    }
                },
                "description": {
                    "type": "search_as_you_type"
                },
                "avatar": {
                    "type": "keyword",
                    "index": false
                },
                "cover": {
                    "type": "keyword",
                    "index": false
                },
                "entity_global_score": {
                    "type": "rank_feature"
                },
                "space_score": {
                    "type": "rank_feature"
                },
                "entity_space_score": {
                    "type": "rank_feature"
                },
                "indexed_at": {
                    "type": "date"
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_settings_structure() {
        let settings = get_index_settings(None);

        // Check settings exist
        assert!(settings["settings"]["number_of_shards"].is_number());
        assert!(settings["settings"]["number_of_replicas"].is_number());

        // Check mappings exist
        assert!(settings["mappings"]["properties"]["entity_id"].is_object());
        assert!(settings["mappings"]["properties"]["name"].is_object());
        assert!(settings["mappings"]["properties"]["description"].is_object());

        // Check search_as_you_type fields
        assert_eq!(
            settings["mappings"]["properties"]["name"]["type"],
            "search_as_you_type"
        );
        assert_eq!(
            settings["mappings"]["properties"]["description"]["type"],
            "search_as_you_type"
        );

        // Check rank_feature fields
        assert_eq!(
            settings["mappings"]["properties"]["entity_global_score"]["type"],
            "rank_feature"
        );
        assert_eq!(
            settings["mappings"]["properties"]["space_score"]["type"],
            "rank_feature"
        );
        assert_eq!(
            settings["mappings"]["properties"]["entity_space_score"]["type"],
            "rank_feature"
        );
    }

    #[test]
    fn test_index_name() {
        assert_eq!(INDEX_NAME, "entities");
    }

    #[test]
    fn test_versioned_index_name() {
        assert_eq!(get_versioned_index_name(None), "entities_v0");
        assert_eq!(get_versioned_index_name(Some(0)), "entities_v0");
        assert_eq!(get_versioned_index_name(Some(1)), "entities_v1");
        assert_eq!(get_versioned_index_name(Some(2)), "entities_v2");
        assert_eq!(get_versioned_index_name(Some(42)), "entities_v42");
    }
}
