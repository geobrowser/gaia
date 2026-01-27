use anyhow::{Context, Result};
use opensearch::OpenSearch;

/// Check if an index exists
pub async fn index_exists(client: &OpenSearch, index_name: &str) -> Result<bool> {
    let response = client
        .indices()
        .exists(opensearch::indices::IndicesExistsParts::Index(&[index_name]))
        .send()
        .await
        .context("Failed to check if index exists")?;

    Ok(response.status_code().is_success())
}
