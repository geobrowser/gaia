//! Search query types for the search indexer.
//!
//! This module defines the query structures used to search the index.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Defines the scope of a search query.
///
/// Different scopes allow users to search globally or within specific
/// space contexts.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SearchScope {
    /// Search across all entities, ranked by text relevance.
    /// This is the default scope.
    #[default]
    Global,

    /// Search globally but include entity-space score breakdowns
    /// showing how different spaces rate each entity.
    GlobalBySpaceScore,

    /// Search within a single space only, excluding subspaces.
    /// Requires `space_ids` with exactly one element.
    SpaceSingle,

    /// Search within a space and all of its subspaces.
    /// Uses the space's web of trust for filtering.
    /// Requires `space_ids` with one or more elements.
    Space,
}

impl SearchScope {
    /// Returns true if this scope requires space_ids.
    pub fn requires_space_ids(&self) -> bool {
        matches!(self, SearchScope::SpaceSingle | SearchScope::Space)
    }
}

/// Search query parameters.
///
/// This struct represents a search request with all necessary parameters
/// to execute a search against the index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    /// The search query string.
    /// Supports fuzzy matching and autocomplete.
    pub query: String,

    /// The scope of the search.
    /// Determines whether to search globally or within a specific space.
    #[serde(default)]
    pub scope: SearchScope,

    /// The space IDs for space-scoped searches.
    /// - For `SpaceSingle`: requires exactly one space ID
    /// - For `Space`: requires one or more space IDs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space_ids: Option<Vec<Uuid>>,

    /// Maximum number of results to return.
    /// Default is 20, maximum is 100.
    #[serde(default = "default_limit")]
    pub limit: usize,

    /// Offset for pagination.
    /// Default is 0.
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize {
    20
}

impl SearchQuery {
    /// Create a new global search query.
    ///
    /// # Arguments
    ///
    /// * `query` - The search query string
    ///
    /// # Example
    ///
    /// ```
    /// use search_indexer_shared::SearchQuery;
    ///
    /// let query = SearchQuery::global("blockchain");
    /// ```
    pub fn global(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            scope: SearchScope::Global,
            space_ids: None,
            limit: default_limit(),
            offset: 0,
        }
    }

    /// Create a new single-space search query.
    ///
    /// # Arguments
    ///
    /// * `query` - The search query string
    /// * `space_id` - The space to search within
    ///
    /// # Example
    ///
    /// ```
    /// use search_indexer_shared::SearchQuery;
    /// use uuid::Uuid;
    ///
    /// let query = SearchQuery::in_space("blockchain", Uuid::new_v4());
    /// ```
    pub fn in_space(query: impl Into<String>, space_id: Uuid) -> Self {
        Self {
            query: query.into(),
            scope: SearchScope::SpaceSingle,
            space_ids: Some(vec![space_id]),
            limit: default_limit(),
            offset: 0,
        }
    }

    /// Create a new multi-space search query (space and subspaces).
    ///
    /// # Arguments
    ///
    /// * `query` - The search query string
    /// * `space_ids` - The space IDs to search within (parent space + subspaces)
    ///
    /// # Example
    ///
    /// ```
    /// use search_indexer_shared::SearchQuery;
    /// use uuid::Uuid;
    ///
    /// let parent = Uuid::new_v4();
    /// let subspace1 = Uuid::new_v4();
    /// let subspace2 = Uuid::new_v4();
    /// let query = SearchQuery::in_spaces("blockchain", vec![parent, subspace1, subspace2]);
    /// ```
    pub fn in_spaces(query: impl Into<String>, space_ids: Vec<Uuid>) -> Self {
        Self {
            query: query.into(),
            scope: SearchScope::Space,
            space_ids: Some(space_ids),
            limit: default_limit(),
            offset: 0,
        }
    }

    /// Set the limit for results.
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit.min(100); // Cap at 100
        self
    }

    /// Set the offset for pagination.
    pub fn with_offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
    }

    /// Validate the query parameters.
    ///
    /// Returns an error message if validation fails.
    pub fn validate(&self) -> Result<(), String> {
        if self.query.trim().is_empty() {
            return Err("Query string cannot be empty".to_string());
        }

        if self.query.len() < 2 {
            return Err("Query must be at least 2 characters".to_string());
        }

        if self.scope.requires_space_ids() {
            match &self.space_ids {
                None => {
                    return Err(format!("space_ids is required for {:?} scope", self.scope));
                }
                Some(ids) if ids.is_empty() => {
                    return Err(format!(
                        "space_ids cannot be empty for {:?} scope",
                        self.scope
                    ));
                }
                Some(ids) if self.scope == SearchScope::SpaceSingle && ids.len() != 1 => {
                    return Err("SpaceSingle scope requires exactly one space_id".to_string());
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Check if the query string looks like a UUID.
    /// Used to determine if we should do a direct ID lookup.
    pub fn is_uuid_query(&self) -> bool {
        Uuid::parse_str(&self.query).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_scope_requires_space_ids() {
        assert!(!SearchScope::Global.requires_space_ids());
        assert!(!SearchScope::GlobalBySpaceScore.requires_space_ids());
        assert!(SearchScope::SpaceSingle.requires_space_ids());
        assert!(SearchScope::Space.requires_space_ids());
    }

    #[test]
    fn test_search_query_global() {
        let query = SearchQuery::global("test");
        assert_eq!(query.query, "test");
        assert_eq!(query.scope, SearchScope::Global);
        assert!(query.space_ids.is_none());
        assert_eq!(query.limit, 20);
        assert_eq!(query.offset, 0);
    }

    #[test]
    fn test_search_query_in_space() {
        let space_id = Uuid::new_v4();
        let query = SearchQuery::in_space("test", space_id);
        assert_eq!(query.scope, SearchScope::SpaceSingle);
        assert_eq!(query.space_ids, Some(vec![space_id]));
    }

    #[test]
    fn test_search_query_in_spaces() {
        let space1 = Uuid::new_v4();
        let space2 = Uuid::new_v4();
        let query = SearchQuery::in_spaces("test", vec![space1, space2]);
        assert_eq!(query.scope, SearchScope::Space);
        assert_eq!(query.space_ids, Some(vec![space1, space2]));
    }

    #[test]
    fn test_search_query_validation() {
        // Valid global query
        let query = SearchQuery::global("test");
        assert!(query.validate().is_ok());

        // Empty query
        let query = SearchQuery::global("");
        assert!(query.validate().is_err());

        // Too short query
        let query = SearchQuery::global("a");
        assert!(query.validate().is_err());

        // Space scope without space_ids
        let mut query = SearchQuery::global("test");
        query.scope = SearchScope::SpaceSingle;
        assert!(query.validate().is_err());

        // Space scope with empty space_ids
        query.space_ids = Some(vec![]);
        assert!(query.validate().is_err());

        // SpaceSingle with multiple space_ids (invalid)
        query.space_ids = Some(vec![Uuid::new_v4(), Uuid::new_v4()]);
        assert!(query.validate().is_err());

        // SpaceSingle with exactly one space_id (valid)
        query.space_ids = Some(vec![Uuid::new_v4()]);
        assert!(query.validate().is_ok());

        // Space with multiple space_ids (valid)
        let mut query = SearchQuery::global("test");
        query.scope = SearchScope::Space;
        query.space_ids = Some(vec![Uuid::new_v4(), Uuid::new_v4()]);
        assert!(query.validate().is_ok());
    }

    #[test]
    fn test_is_uuid_query() {
        let query = SearchQuery::global("550e8400-e29b-41d4-a716-446655440000");
        assert!(query.is_uuid_query());

        let query = SearchQuery::global("blockchain");
        assert!(!query.is_uuid_query());
    }

    #[test]
    fn test_with_limit_caps_at_100() {
        let query = SearchQuery::global("test").with_limit(200);
        assert_eq!(query.limit, 100);
    }
}
