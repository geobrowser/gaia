//! Index and alias management for OpenSearch.
//!
//! This module provides functions for creating and managing OpenSearch indices and aliases.

use opensearch::{
    indices::{IndicesCreateParts, IndicesExistsParts, IndicesGetAliasParts},
    OpenSearch,
};
use serde_json::{json, Value};
use tracing::{debug, error, info, warn};

use crate::errors::SearchIndexError;
use crate::opensearch::index_config::{get_index_settings, get_versioned_index_name, IndexConfig};

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
    // Get the versioned index name (e.g., "entities_v0")
    let versioned_index_name = get_versioned_index_name(Some(index_config.version));

    // Step 1: Ensure the versioned index exists
    ensure_versioned_index_exists(client, &versioned_index_name, index_config.version).await?;

    // Step 2: Ensure the alias points to the correct index
    ensure_alias_points_to_index(client, &index_config.alias, &versioned_index_name).await?;

    Ok(())
}

/// Check if automatic index creation is allowed.
/// Index creation is only allowed if:
/// - version is 0 (default development version), OR
/// - the `auto_index_creation` feature is enabled at compile time
fn should_allow_index_creation(version: u32) -> bool {
    if version == 0 {
        return true;
    }

    // Check if the auto_index_creation feature is enabled
    #[cfg(feature = "auto_index_creation")]
    return true;

    #[cfg(not(feature = "auto_index_creation"))]
    false
}

/// Ensure the versioned index exists, creating it if necessary.
///
/// Note: Automatic index creation is only allowed for version 0 or when the
/// `auto_index_creation` feature is enabled. In production, indices should be
/// created using the search-admin-cli tool.
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

    if !index_exists_response.status_code().is_success() {
        // Index doesn't exist - check if we should create it automatically
        if !should_allow_index_creation(version) {
            error!(
                index = %versioned_index_name,
                version = version,
                "Index does not exist and automatic creation is disabled. \
                 The index should be created using the search-admin-cli tool."
            );
            return Err(SearchIndexError::index_creation(format!(
                "Index '{}' does not exist. Automatic index creation is only allowed for version 0 \
                 or when compiled with the 'auto_index_creation' feature. \
                 The index should be created using the search-admin-cli tool.",
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

        let status = create_response.status_code();
        if !status.is_success() {
            let error_body = create_response.text().await.unwrap_or_default();
            error!(status = %status, body = %error_body, "Index creation failed");
            return Err(SearchIndexError::index_creation(format!(
                "Index creation failed with status {}: {}",
                status, error_body
            )));
        }

        info!(index = %versioned_index_name, "Versioned index created successfully");
    } else {
        debug!(index = %versioned_index_name, "Versioned index already exists");
    }

    Ok(())
}

/// Ensure the alias points to the correct versioned index.
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
        // Alias doesn't exist, create it
        create_alias(client, alias, versioned_index_name).await
    } else {
        // Alias exists, check if it points to the correct index
        let alias_body: Value = get_alias_response
            .map_err(|e| {
                SearchIndexError::connection(format!("Failed to get alias response: {}", e))
            })?
            .json()
            .await
            .map_err(|e| SearchIndexError::parse(e.to_string()))?;

        // Check if alias points to the versioned index
        let points_to_correct_index = alias_body
            .as_object()
            .and_then(|obj| obj.get(versioned_index_name))
            .is_some();

        if !points_to_correct_index {
            update_alias_to_correct_index(client, alias, versioned_index_name, &alias_body).await
        } else {
            debug!(alias = %alias, index = %versioned_index_name, "Alias already points to correct index");
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

/// Update an existing alias to point to the correct versioned index.
async fn update_alias_to_correct_index(
    client: &OpenSearch,
    alias: &str,
    versioned_index_name: &str,
    current_alias_body: &Value,
) -> Result<(), SearchIndexError> {
    // First, remove alias from all indices it currently points to
    warn!(
        alias = %alias,
        expected_index = %versioned_index_name,
        "Alias points to different index, updating"
    );

    let mut actions = Vec::new();
    if let Some(indices) = current_alias_body.as_object() {
        for index_name in indices.keys() {
            actions.push(json!({
                "remove": {
                    "index": index_name,
                    "alias": alias
                }
            }));
        }
    }
    // Then add alias to the correct index
    actions.push(json!({
        "add": {
            "index": versioned_index_name,
            "alias": alias
        }
    }));

    let update_response = client
        .indices()
        .update_aliases()
        .body(json!({ "actions": actions }))
        .send()
        .await
        .map_err(|e| SearchIndexError::index_creation(e.to_string()))?;

    let status = update_response.status_code();
    if !status.is_success() {
        let error_body = update_response.text().await.unwrap_or_default();
        error!(status = %status, body = %error_body, "Alias update failed");
        return Err(SearchIndexError::index_creation(format!(
            "Alias update failed with status {}: {}",
            status, error_body
        )));
    }

    info!(alias = %alias, index = %versioned_index_name, "Alias updated successfully");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_allow_index_creation_version_0() {
        // Version 0 should always allow creation regardless of feature flags
        assert!(should_allow_index_creation(0));
    }

    #[test]
    #[cfg(feature = "auto_index_creation")]
    fn test_should_allow_index_creation_with_feature() {
        // When feature is enabled, version > 0 should allow creation
        assert!(should_allow_index_creation(1));
        assert!(should_allow_index_creation(100));
    }

    #[test]
    #[cfg(not(feature = "auto_index_creation"))]
    fn test_should_disallow_index_creation_without_feature() {
        // When feature is disabled (default), version > 0 should not allow creation
        assert!(!should_allow_index_creation(1));
        assert!(!should_allow_index_creation(100));
    }
}
