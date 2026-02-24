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

/// Get a versioned index name with a custom base name.
///
/// This allows generating versioned index names with environment prefixes.
///
/// # Arguments
///
/// * `base_name` - The base index name (e.g., "staging_entities" or "entities")
/// * `version` - The version number (defaults to 0 if None)
///
/// # Returns
///
/// The versioned index name (e.g., "staging_entities_v2")
///
/// # Example
///
/// ```
/// use search_indexer_repository::opensearch::get_versioned_index_name_with_base;
///
/// assert_eq!(get_versioned_index_name_with_base("entities", Some(2)), "entities_v2");
/// assert_eq!(get_versioned_index_name_with_base("staging_entities", Some(2)), "staging_entities_v2");
/// ```
pub fn get_versioned_index_name_with_base(base_name: &str, version: Option<u32>) -> String {
    let v = version.unwrap_or(0);
    format!("{}_v{}", base_name, v)
}

/// Get the index settings and mappings for the entity search index.
///
/// The configuration includes:
/// - **search_as_you_type**: Built-in field type for autocomplete on name and description
/// - **float**: Score fields that support zero, negative, and positive values
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
                "image_url": {
                    "type": "keyword",
                    "index": false
                },
                "relations": {
                    "type": "nested",
                    "properties": {
                        "relation_id": {
                            "type": "keyword"
                        },
                        "relation_type": {
                            "type": "keyword"
                        },
                        "to_entity_id": {
                            "type": "keyword"
                        }
                    }
                },
                "entity_global_score": {
                    "type": "float"
                },
                "space_score": {
                    "type": "float"
                },
                "entity_space_score": {
                    "type": "float"
                },
                "space_topic_entity_id": {
                    "type": "keyword"
                },
                "indexed_at": {
                    "type": "date"
                },
                "deleted": {
                    "type": "boolean"
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
        assert!(settings["mappings"]["properties"]["relations"].is_object());

        // Check search_as_you_type fields
        assert_eq!(
            settings["mappings"]["properties"]["name"]["type"],
            "search_as_you_type"
        );
        assert_eq!(
            settings["mappings"]["properties"]["description"]["type"],
            "search_as_you_type"
        );

        // Check relations nested type
        assert_eq!(
            settings["mappings"]["properties"]["relations"]["type"],
            "nested"
        );
        assert_eq!(
            settings["mappings"]["properties"]["relations"]["properties"]["relation_id"]
                ["type"],
            "keyword"
        );
        assert_eq!(
            settings["mappings"]["properties"]["relations"]["properties"]["relation_type"]
                ["type"],
            "keyword"
        );
        assert_eq!(
            settings["mappings"]["properties"]["relations"]["properties"]["to_entity_id"]
                ["type"],
            "keyword"
        );

        // Check float score fields
        assert_eq!(
            settings["mappings"]["properties"]["entity_global_score"]["type"],
            "float"
        );
        assert_eq!(
            settings["mappings"]["properties"]["space_score"]["type"],
            "float"
        );
        assert_eq!(
            settings["mappings"]["properties"]["entity_space_score"]["type"],
            "float"
        );
    }

    #[test]
    fn test_versioned_index_name_with_base() {
        // Production (no prefix)
        assert_eq!(
            get_versioned_index_name_with_base("entities", None),
            "entities_v0"
        );
        assert_eq!(
            get_versioned_index_name_with_base("entities", Some(2)),
            "entities_v2"
        );

        // Staging (with prefix)
        assert_eq!(
            get_versioned_index_name_with_base("staging_entities", None),
            "staging_entities_v0"
        );
        assert_eq!(
            get_versioned_index_name_with_base("staging_entities", Some(2)),
            "staging_entities_v2"
        );
    }
}
