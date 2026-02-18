//! Index and alias management for OpenSearch.
//!
//! This module provides functions for creating and managing OpenSearch indices and aliases.

use opensearch::{
    indices::{IndicesCreateParts, IndicesExistsParts, IndicesGetAliasParts},
    OpenSearch,
};
use serde_json::{json, Value};
use tracing::{error, info};

use crate::errors::SearchIndexError;
use crate::opensearch::index_config::{get_index_settings, get_versioned_index_name_with_base, IndexConfig};

/// Ensure the versioned index and alias exist, creating them if necessary.
///
/// This function performs the following steps:
/// 1. Check if the versioned index (e.g., "entities_v0") exists; create it if not
/// 2. Check if the alias (e.g., "entities") exists and points to the correct index
/// 3. Create or update the alias to point to the versioned index
///
/// # Arguments
///
/// * `client` - The OpenSearch client
/// * `index_config` - Configuration containing alias name and version
///
/// # Returns
///
/// * `Ok(())` - If the index and alias are ready for use
/// * `Err(SearchIndexError)` - If index or alias operations fail
pub async fn ensure_index_exists(
    client: &OpenSearch,
    index_config: &IndexConfig,
) -> Result<(), SearchIndexError> {
    // Get the versioned index name (e.g., "staging_entities_v2" or "entities_v2")
    let versioned_index_name = get_versioned_index_name_with_base(&index_config.alias, Some(index_config.version));

    // Step 1: Ensure the versioned index exists
    ensure_versioned_index_exists(client, &versioned_index_name, index_config.version).await?;

    // Step 2: Ensure the alias points to the correct index
    ensure_alias_points_to_index(client, &index_config.alias, &versioned_index_name).await?;

    Ok(())
}

/// Check if automatic index and alias changes are allowed.
/// Only allowed if the `auto_index_creation` feature is enabled at compile time.
fn should_allow_index_and_alias_changes() -> bool {
    #[cfg(feature = "auto_index_creation")]
    return true;

    #[cfg(not(feature = "auto_index_creation"))]
    false
}

/// Ensure the versioned index exists, creating it if necessary.
///
/// Note: Automatic index creation is only allowed when the `auto_index_creation`
/// feature is enabled. In production, indices should be created using the search-admin tool.
async fn ensure_versioned_index_exists(
    client: &OpenSearch,
    versioned_index_name: &str,
    version: u32,
) -> Result<(), SearchIndexError> {
    let index_exists_response = client
        .indices()
        .exists(IndicesExistsParts::Index(&[versioned_index_name]))
        .send()
        .await
        .map_err(|e| SearchIndexError::connection(e.to_string()))?;

    let status = index_exists_response.status_code();

    if !status.is_success() {
        // Check if this is specifically a 404 (index not found)
        if status.as_u16() == 404 {
            // Index doesn't exist - check if we should create it automatically
            if !should_allow_index_and_alias_changes() {
                error!(
                    index = %versioned_index_name,
                    version = version,
                    "Index does not exist and automatic creation is disabled. \
                     The index should be created using the search-admin tool."
                );
                return Err(SearchIndexError::index_creation(format!(
                    "Index '{}' does not exist. Automatic index creation requires the \
                     'auto_index_creation' feature. The index should be created using the search-admin tool.",
                    versioned_index_name
                )));
            }

            info!(index = %versioned_index_name, version = version, "Creating versioned index");

            let settings = get_index_settings(Some(version));

            let create_response = client
                .indices()
                .create(IndicesCreateParts::Index(versioned_index_name))
                .body(settings)
                .send()
                .await
                .map_err(|e| SearchIndexError::index_creation(e.to_string()))?;

            let create_status = create_response.status_code();
            if !create_status.is_success() {
                let error_body = create_response.text().await.unwrap_or_default();
                error!(status = %create_status, body = %error_body, "Index creation failed");
                return Err(SearchIndexError::index_creation(format!(
                    "Index creation failed with status {}: {}",
                    create_status, error_body
                )));
            }

            info!(index = %versioned_index_name, "Versioned index created successfully");
        } else {
            // Non-404 error (e.g., 401, 403, 500) - fail with error
            let error_body = index_exists_response.text().await.unwrap_or_default();
            error!(
                index = %versioned_index_name,
                status = %status,
                body = %error_body,
                "Failed to check if index exists"
            );
            return Err(SearchIndexError::index_creation(format!(
                "Failed to check if index '{}' exists: status {} - {}",
                versioned_index_name, status, error_body
            )));
        }
    } else {
        info!(index = %versioned_index_name, "Index exists");
    }

    Ok(())
}

/// Ensure the alias points to the correct versioned index.
///
/// If the `auto_index_creation` feature is enabled, this will create the alias if it
/// doesn't exist. If the alias exists and points to a different index, this will fail.
///
/// If the feature is not enabled, this will fail if the alias doesn't point to the
/// correct index. Use the search-admin tool to manage aliases in production.
async fn ensure_alias_points_to_index(
    client: &OpenSearch,
    alias: &str,
    versioned_index_name: &str,
) -> Result<(), SearchIndexError> {
    // Use get_alias to check if alias exists and what it points to
    let get_alias_response = client
        .indices()
        .get_alias(IndicesGetAliasParts::Name(&[alias]))
        .send()
        .await;

    let alias_exists = get_alias_response.is_ok()
        && get_alias_response
            .as_ref()
            .map(|resp| resp.status_code().is_success())
            .unwrap_or(false);

    if !alias_exists {
        // Alias doesn't exist
        if should_allow_index_and_alias_changes() {
            // Feature enabled, create the alias
            create_alias(client, alias, versioned_index_name).await
        } else {
            // Feature not enabled, fail - alias should be managed by search-admin
            error!(
                alias = %alias,
                expected_index = %versioned_index_name,
                "Alias does not exist. Use search-admin to create the alias."
            );
            Err(SearchIndexError::index_creation(format!(
                "Alias '{}' does not exist. Automatic alias creation requires the \
                 'auto_index_creation' feature. Use the search-admin tool to create the alias.",
                alias
            )))
        }
    } else {
        // Alias exists, check if it points to the correct index
        let alias_body: Value = get_alias_response
            .map_err(|e| {
                SearchIndexError::connection(format!("Failed to get alias response: {}", e))
            })?
            .json()
            .await
            .map_err(|e| SearchIndexError::parse(e.to_string()))?;

        // Check if alias points ONLY to the versioned index (not multiple indices)
        let indices: Vec<&str> = alias_body
            .as_object()
            .map(|obj| obj.keys().map(|s| s.as_str()).collect())
            .unwrap_or_default();

        let points_to_correct_index = indices.len() == 1 && indices[0] == versioned_index_name;

        if !points_to_correct_index {
            // Alias points to wrong index - always fail, even with feature enabled
            // Use search-admin to explicitly update the alias
            error!(
                alias = %alias,
                expected_index = %versioned_index_name,
                current_indices = ?indices,
                "Alias points to wrong index(es). Use search-admin to update the alias."
            );
            Err(SearchIndexError::index_creation(format!(
                "Alias '{}' points to {:?} but expected '{}'. Use the search-admin tool to \
                 update the alias.",
                alias, indices, versioned_index_name
            )))
        } else {
            info!(alias = %alias, index = %versioned_index_name, "Alias points to correct index");
            Ok(())
        }
    }
}

/// Create a new alias pointing to the versioned index.
async fn create_alias(
    client: &OpenSearch,
    alias: &str,
    versioned_index_name: &str,
) -> Result<(), SearchIndexError> {
    info!(alias = %alias, index = %versioned_index_name, "Creating alias");

    let actions = json!({
        "actions": [
            {
                "add": {
                    "index": versioned_index_name,
                    "alias": alias
                }
            }
        ]
    });

    let update_response = client
        .indices()
        .update_aliases()
        .body(actions)
        .send()
        .await
        .map_err(|e| SearchIndexError::index_creation(e.to_string()))?;

    let status = update_response.status_code();
    if !status.is_success() {
        let error_body = update_response.text().await.unwrap_or_default();
        error!(status = %status, body = %error_body, "Alias creation failed");
        return Err(SearchIndexError::index_creation(format!(
            "Alias creation failed with status {}: {}",
            status, error_body
        )));
    }

    info!(alias = %alias, index = %versioned_index_name, "Alias created successfully");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "auto_index_creation")]
    fn test_should_allow_index_and_alias_changes_with_feature() {
        assert!(should_allow_index_and_alias_changes());
    }

    #[test]
    #[cfg(not(feature = "auto_index_creation"))]
    fn test_should_disallow_index_and_alias_changes_without_feature() {
        assert!(!should_allow_index_and_alias_changes());
    }
}
