//! Client for embedding-service. Every response's descriptor hash is checked against the slot's:
//! a vector from a different descriptor is never written.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    /// The service's request limits, read from `/info` by [`verify`](Self::verify). Texts are
    /// capped to `max_text_chars` *before* hashing and batches to `max_batch`, so a document
    /// can never make the service answer 413 — which the client treats as fatal, because
    /// after this clamp it can only mean a misconfiguration.
    max_text_chars: Arc<AtomicUsize>,
    max_batch: Arc<AtomicUsize>,
}

/// Defaults when `/info` does not advertise limits (an older service).
pub const DEFAULT_MAX_TEXT_CHARS: usize = 8000;
pub const DEFAULT_MAX_BATCH: usize = 256;

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
            max_text_chars: Arc::new(AtomicUsize::new(DEFAULT_MAX_TEXT_CHARS)),
            max_batch: Arc::new(AtomicUsize::new(DEFAULT_MAX_BATCH)),
        })
    }

    /// Longest text (in chars) the service accepts per item.
    pub fn max_text_chars(&self) -> usize {
        self.max_text_chars.load(Ordering::Relaxed)
    }

    /// Most texts the service accepts per request.
    pub fn max_batch(&self) -> usize {
        self.max_batch.load(Ordering::Relaxed)
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
                if let Some(n) = info["limits"]["max_text_chars"].as_u64() {
                    self.max_text_chars
                        .store(n.max(1) as usize, Ordering::Relaxed);
                }
                if let Some(n) = info["limits"]["max_batch"].as_u64() {
                    self.max_batch.store(n.max(1) as usize, Ordering::Relaxed);
                }
                info!(
                    slot = %self.slot, service = %self.base,
                    max_text_chars = self.max_text_chars(), max_batch = self.max_batch(),
                    "embedding-service verified"
                );
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
