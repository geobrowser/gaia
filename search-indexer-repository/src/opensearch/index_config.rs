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
            "number_of_replicas": 1,
            "analysis": {
                "char_filter": {
                    // Folds the Unicode apostrophe variants onto ASCII U+0027 so that a
                    // query typed with one spelling reaches text written with another.
                    //
                    // The standard tokenizer keeps an apostrophe inside the token it
                    // appears in (UAX #29 treats it as MidLetter), so "man\u{2019}s" and
                    // "man's" index as two entirely unrelated terms. The corpus is mixed —
                    // editors paste from sources that smart-quote and from sources that do
                    // not — so without this fold, whether a search works is decided by
                    // which apostrophe the author happened to use. Measured on testnet
                    // 2026-09-15: `man's` scored 0.00 against a claim containing
                    // "man\u{2019}s", while the same query with U+2019 scored 401. See
                    // GEO-2904.
                    "apostrophe_fold": {
                        "type": "mapping",
                        "mappings": [
                            "\u{2019} => \u{0027}",
                            "\u{2018} => \u{0027}",
                            "\u{02BC} => \u{0027}",
                            "\u{FF07} => \u{0027}"
                        ]
                    }
                },
                "analyzer": {
                    // The default `standard` analyzer (tokenize + lowercase) with the
                    // apostrophe fold applied before tokenization. Deliberately does NOT
                    // stem: `name` and `description` are search_as_you_type, and their
                    // prefix sub-fields index prefixes of the *indexed* term, so stemming
                    // here would break autocomplete on partially typed words ("runn" no
                    // longer prefixes "running" once it stems to "run"). Stemming belongs
                    // on sibling fields, not on these.
                    "text_apostrophe_folded": {
                        "type": "custom",
                        "char_filter": ["apostrophe_fold"],
                        "tokenizer": "standard",
                        "filter": ["lowercase"]
                    }
                },
                "normalizer": {
                    // Keyword-field counterpart for `name_raw`. Applies the same fold with
                    // no lowercase filter, because the exact-match clause on name_raw is
                    // case-sensitive by design (see NAME_RAW_EXACT_BOOST in the API's
                    // opensearch.ts) and a lowercase normalizer would silently erase that.
                    "apostrophe_folded_keyword": {
                        "type": "custom",
                        "char_filter": ["apostrophe_fold"]
                    }
                }
            }
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
                    "analyzer": "text_apostrophe_folded"
                },
                "name_raw": {
                    "type": "keyword",
                    "normalizer": "apostrophe_folded_keyword"
                },
                "description": {
                    "type": "search_as_you_type",
                    "analyzer": "text_apostrophe_folded"
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
                },
                "in_canonical_graph": {
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
            settings["mappings"]["properties"]["relations"]["properties"]["relation_id"]["type"],
            "keyword"
        );
        assert_eq!(
            settings["mappings"]["properties"]["relations"]["properties"]["relation_type"]["type"],
            "keyword"
        );
        assert_eq!(
            settings["mappings"]["properties"]["relations"]["properties"]["to_entity_id"]["type"],
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
    fn test_apostrophe_fold_is_wired_into_text_fields() {
        let settings = get_index_settings(None);
        let analysis = &settings["settings"]["analysis"];

        // The fold must map every Unicode apostrophe variant onto ASCII U+0027.
        let mappings = analysis["char_filter"]["apostrophe_fold"]["mappings"]
            .as_array()
            .expect("apostrophe_fold.mappings should be an array");
        let rules: Vec<&str> = mappings.iter().map(|m| m.as_str().unwrap()).collect();
        assert_eq!(
            analysis["char_filter"]["apostrophe_fold"]["type"],
            "mapping"
        );
        assert!(
            rules.contains(&"\u{2019} => \u{0027}"),
            "U+2019 must fold: {rules:?}"
        );
        assert!(
            rules.contains(&"\u{2018} => \u{0027}"),
            "U+2018 must fold: {rules:?}"
        );
        assert!(
            rules.contains(&"\u{02BC} => \u{0027}"),
            "U+02BC must fold: {rules:?}"
        );
        assert!(
            rules.contains(&"\u{FF07} => \u{0027}"),
            "U+FF07 must fold: {rules:?}"
        );

        // The analyzer applies the fold before the standard tokenizer, and lowercases.
        let analyzer = &analysis["analyzer"]["text_apostrophe_folded"];
        assert_eq!(analyzer["tokenizer"], "standard");
        assert_eq!(analyzer["char_filter"][0], "apostrophe_fold");
        assert_eq!(analyzer["filter"][0], "lowercase");

        // It must NOT stem — stemming would break search_as_you_type prefix matching.
        let filters = analyzer["filter"].as_array().unwrap();
        assert_eq!(filters.len(), 1, "only lowercase belongs here: {filters:?}");

        // Both analyzed text fields must use it, or the fold is half-applied.
        let props = &settings["mappings"]["properties"];
        assert_eq!(props["name"]["analyzer"], "text_apostrophe_folded");
        assert_eq!(props["description"]["analyzer"], "text_apostrophe_folded");
    }

    #[test]
    fn test_name_raw_folds_apostrophes_but_preserves_case() {
        let settings = get_index_settings(None);
        let normalizer =
            &settings["settings"]["analysis"]["normalizer"]["apostrophe_folded_keyword"];

        assert_eq!(normalizer["char_filter"][0], "apostrophe_fold");
        assert_eq!(
            settings["mappings"]["properties"]["name_raw"]["normalizer"],
            "apostrophe_folded_keyword"
        );

        // name_raw backs a deliberately case-SENSITIVE term clause
        // (NAME_RAW_EXACT_BOOST in the API). A lowercase filter here would erase
        // that distinction silently, so its absence is load-bearing.
        let filters = normalizer["filter"].as_array();
        assert!(
            filters.is_none_or(|f| f.is_empty()),
            "name_raw normalizer must not lowercase: {filters:?}"
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
