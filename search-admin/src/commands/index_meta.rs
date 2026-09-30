//! Shared I/O for the embedding-slot commands: resolve which index to act on, read its settings
//! and `_meta`, and write mappings.

use anyhow::{Context, Result, bail};
use opensearch::OpenSearch;
use serde_json::Value;
use tracing::info;

use crate::commands::get;

/// `--version N` → `<alias>_vN` (must exist); no version → the single index behind the alias.
pub async fn resolve_index(
    client: &OpenSearch,
    index_alias: &str,
    version: Option<u32>,
) -> Result<String> {
    if let Some(v) = version {
        let name = format!("{index_alias}_v{v}");
        if !get::index_exists(client, &name).await? {
            bail!("index {name} does not exist");
        }
        return Ok(name);
    }
    let response = client
        .indices()
        .get_alias(opensearch::indices::IndicesGetAliasParts::Name(&[
            index_alias,
        ]))
        .send()
        .await
        .context("Failed to resolve alias")?;
    if !response.status_code().is_success() {
        bail!("alias {index_alias} does not exist (pass --version to name an index directly)");
    }
    let body: Value = response
        .json()
        .await
        .context("Failed to parse alias response")?;
    let mut names: Vec<String> = body
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    names.sort();
    match names.as_slice() {
        [one] => Ok(one.clone()),
        [] => bail!("alias {index_alias} points at no index"),
        many => bail!(
            "alias {index_alias} points at {} indices ({}); pass --version",
            many.len(),
            many.join(", ")
        ),
    }
}

/// The index's mappings object (`properties`, `_meta`, ...).
pub async fn get_mappings(client: &OpenSearch, index: &str) -> Result<Value> {
    let response = client
        .indices()
        .get_mapping(opensearch::indices::IndicesGetMappingParts::Index(&[index]))
        .send()
        .await
        .context("Failed to get mapping")?;
    if !response.status_code().is_success() {
        bail!(
            "get mapping failed: {}",
            response.text().await.unwrap_or_default()
        );
    }
    let body: Value = response
        .json()
        .await
        .context("Failed to parse mapping response")?;
    Ok(body[index]["mappings"].clone())
}

pub fn meta_of(mappings: &Value) -> Option<Value> {
    mappings.get("_meta").filter(|m| m.is_object()).cloned()
}

/// Whether `index.knn` is on. It is a static setting: an index created without it cannot serve
/// the `knn` query, and the only fix is a new index version plus a reindex.
pub async fn knn_enabled(client: &OpenSearch, index: &str) -> Result<bool> {
    let response = client
        .indices()
        .get_settings(opensearch::indices::IndicesGetSettingsParts::Index(&[
            index,
        ]))
        .send()
        .await
        .context("Failed to get settings")?;
    if !response.status_code().is_success() {
        bail!(
            "get settings failed: {}",
            response.text().await.unwrap_or_default()
        );
    }
    let body: Value = response
        .json()
        .await
        .context("Failed to parse settings response")?;
    let knn = &body[index]["settings"]["index"]["knn"];
    Ok(knn == "true" || knn == true)
}

pub async fn put_mapping(client: &OpenSearch, index: &str, body: Value) -> Result<()> {
    info!(index = %index, "put mapping");
    let response = client
        .indices()
        .put_mapping(opensearch::indices::IndicesPutMappingParts::Index(&[index]))
        .body(body)
        .send()
        .await
        .context("Failed to put mapping")?;
    if !response.status_code().is_success() {
        bail!(
            "put mapping failed: {}",
            response.text().await.unwrap_or_default()
        );
    }
    Ok(())
}

/// Documents matching a query body (`{"query": ...}`).
pub async fn count(client: &OpenSearch, index: &str, query: Value) -> Result<u64> {
    let response = client
        .count(opensearch::CountParts::Index(&[index]))
        .body(query)
        .send()
        .await
        .context("Failed to count")?;
    if !response.status_code().is_success() {
        bail!(
            "count failed: {}",
            response.text().await.unwrap_or_default()
        );
    }
    let body: Value = response
        .json()
        .await
        .context("Failed to parse count response")?;
    Ok(body["count"].as_u64().unwrap_or(0))
}

/// `GET <embedding-service>/info` → `slot id → descriptor_hash` for every loaded slot.
pub async fn service_loaded_slots(url: &str) -> Result<std::collections::BTreeMap<String, String>> {
    let info: Value = reqwest::Client::new()
        .get(format!("{}/info", url.trim_end_matches('/')))
        .send()
        .await
        .with_context(|| format!("GET {url}/info"))?
        .error_for_status()
        .with_context(|| format!("GET {url}/info"))?
        .json()
        .await
        .context("parse /info")?;
    let mut out = std::collections::BTreeMap::new();
    if let Some(slots) = info["slots"].as_object() {
        for (id, s) in slots {
            if let Some(h) = s["descriptor_hash"].as_str() {
                out.insert(id.clone(), h.to_string());
            }
        }
    }
    Ok(out)
}
