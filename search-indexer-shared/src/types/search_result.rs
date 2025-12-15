//! Search result types for the search indexer.
//!
//! This module defines the response structures returned from search operations.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A single search result item.
///
/// Contains the entity data along with its relevance score from the search engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    /// The entity's unique identifier.
    pub entity_id: Uuid,

    /// The space this entity belongs to.
    pub space_id: Uuid,

    /// Optional entity display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Optional description text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Optional avatar image URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,

    /// Optional cover image URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,

    /// Global entity score (None until scoring service is implemented).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_global_score: Option<f64>,

    /// Space score (None until scoring service is implemented).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space_score: Option<f64>,

    /// Entity-space score (None until scoring service is implemented).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_space_score: Option<f64>,

    /// Relevance score from the search engine.
    /// Higher scores indicate better matches.
    pub relevance_score: f64,
}

/// Complete search response with results and metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResponse {
    /// The list of search results, ordered by relevance.
    pub results: Vec<SearchResult>,

    /// Total number of matching documents.
    /// May be greater than the number of returned results due to pagination.
    pub total: u64,

    /// Time taken to execute the search in milliseconds.
    pub took_ms: u64,
}

impl SearchResponse {
    /// Create an empty search response.
    pub fn empty() -> Self {
        Self {
            results: Vec::new(),
            total: 0,
            took_ms: 0,
        }
    }

    /// Create a new search response.
    pub fn new(results: Vec<SearchResult>, total: u64, took_ms: u64) -> Self {
        Self {
            results,
            total,
            took_ms,
        }
    }

    /// Returns true if there are no results.
    pub fn is_empty(&self) -> bool {
        self.results.is_empty()
    }

    /// Returns the number of results in this response.
    pub fn len(&self) -> usize {
        self.results.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_response_empty() {
        let response = SearchResponse::empty();
        assert!(response.is_empty());
        assert_eq!(response.len(), 0);
        assert_eq!(response.total, 0);
    }

    #[test]
    fn test_search_response_new() {
        let results = vec![SearchResult {
            entity_id: Uuid::new_v4(),
            space_id: Uuid::new_v4(),
            name: Some("Test".to_string()),
            description: None,
            avatar: None,
            cover: None,
            entity_global_score: None,
            space_score: None,
            entity_space_score: None,
            relevance_score: 1.5,
        }];

        let response = SearchResponse::new(results, 100, 5);
        assert!(!response.is_empty());
        assert_eq!(response.len(), 1);
        assert_eq!(response.total, 100);
        assert_eq!(response.took_ms, 5);
    }

    #[test]
    fn test_serialization() {
        let response = SearchResponse::new(
            vec![SearchResult {
                entity_id: Uuid::new_v4(),
                space_id: Uuid::new_v4(),
                name: Some("Test".to_string()),
                description: Some("Description".to_string()),
                avatar: None,
                cover: None,
                entity_global_score: None,
                space_score: None,
                entity_space_score: None,
                relevance_score: 2.5,
            }],
            1,
            10,
        );

        let json = serde_json::to_string(&response).unwrap();
        let deserialized: SearchResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(response.total, deserialized.total);
        assert_eq!(response.results.len(), deserialized.results.len());
    }
}

