//! Client for embedding-service. Every response's descriptor hash is checked against the slot's:
//! a vector from a different descriptor is never written.

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

use crate::error::{IndexerError, Result};

#[derive(Clone)]
pub struct ServiceClient {
    http: reqwest::Client,
    base: String,
    slot: String,
    expected_hash: String,
    dimensions: usize,
}

#[derive(Deserialize)]
struct EmbedResponse {
    descriptor_hash: String,
    dimensions: usize,
    vectors: Vec<Vec<f32>>,
}

impl ServiceClient {
    pub fn new(base: &str, slot: &str, expected_hash: &str, dimensions: usize) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(IndexerError::fatal)?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            slot: slot.to_string(),
            expected_hash: expected_hash.to_string(),
            dimensions,
        })
    }

    /// `GET /info` must list the slot with the descriptor hash the index registered.
    pub async fn verify(&self) -> Result<()> {
        let info: serde_json::Value = self
            .http
            .get(format!("{}/info", self.base))
            .send()
            .await
            .map_err(IndexerError::transient)?
            .error_for_status()
            .map_err(IndexerError::transient)?
            .json()
            .await
            .map_err(IndexerError::transient)?;
        match info["slots"][&self.slot]["descriptor_hash"].as_str() {
            None => Err(IndexerError::Fatal(format!(
                "embedding-service at {} does not have slot {} loaded",
                self.base, self.slot
            ))),
            Some(h) if h != self.expected_hash => Err(IndexerError::Fatal(format!(
                "embedding-service slot {} has descriptor hash {h}, the index registered {}",
                self.slot, self.expected_hash
            ))),
            Some(_) => {
                info!(slot = %self.slot, service = %self.base, "embedding-service verified");
                Ok(())
            }
        }
    }

    /// Document embeddings, in order. Transient on network or 5xx; fatal on a descriptor or
    /// dimension mismatch (a bad slot config, never something a retry fixes).
    pub async fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let response = self
            .http
            .post(format!("{}/embed", self.base))
            .json(&json!({ "slot": self.slot, "purpose": "document", "texts": texts }))
            .send()
            .await
            .map_err(IndexerError::transient)?;
        let status = response.status();
        if status.is_client_error() {
            let body = response.text().await.unwrap_or_default();
            return Err(IndexerError::Fatal(format!(
                "embedding-service rejected the batch ({status}): {body}"
            )));
        }
        let response = response
            .error_for_status()
            .map_err(IndexerError::transient)?;
        let parsed: EmbedResponse = response.json().await.map_err(IndexerError::transient)?;
        if parsed.descriptor_hash != self.expected_hash {
            return Err(IndexerError::Fatal(format!(
                "embedding-service answered with descriptor {}, expected {}",
                parsed.descriptor_hash, self.expected_hash
            )));
        }
        if parsed.dimensions != self.dimensions
            || parsed.vectors.iter().any(|v| v.len() != self.dimensions)
        {
            return Err(IndexerError::Fatal(format!(
                "embedding-service answered with {} dimensions, expected {}",
                parsed.dimensions, self.dimensions
            )));
        }
        if parsed.vectors.len() != texts.len() {
            warn!(
                sent = texts.len(),
                got = parsed.vectors.len(),
                "vector count mismatch"
            );
            return Err(IndexerError::Transient("vector count mismatch".into()));
        }
        Ok(parsed.vectors)
    }
}
