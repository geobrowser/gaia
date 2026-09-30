//! OpenSearch access: the slot's descriptor from `_meta`, paged searches, bulk scripted updates,
//! and the per-slot control document.

use std::time::Instant;

use embedding::Descriptor;
use opensearch::http::request::JsonBody;
use opensearch::http::transport::{SingleNodeConnectionPool, TransportBuilder};
use opensearch::{BulkParts, OpenSearch, SearchParts};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{debug, warn};

use crate::error::{IndexerError, Result};

pub struct Store {
    client: OpenSearch,
    pub index: String,
    pub control_index: String,
}

#[derive(Debug, Default, Clone)]
pub struct BulkOutcome {
    pub updated: usize,
    pub noop: usize,
    pub missing: usize,
    pub failed: Vec<String>,
    pub took_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Backfill,
    Follow,
}

/// One document per slot in the control index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlDoc {
    pub slot: String,
    pub mode: Mode,
    /// Follow mode: newest `indexed_at` (epoch ms) fully processed.
    pub checkpoint_ms: Option<i64>,
    /// Backfill mode: sort values of the last processed document.
    pub backfill_after: Option<Value>,
    pub backfill_started_ms: Option<i64>,
    pub updated_at: String,
    #[serde(default)]
    pub last_cycle: Value,
}

fn transient_status(status: opensearch::http::StatusCode) -> bool {
    status.as_u16() == 429 || status.is_server_error()
}

impl Store {
    pub fn new(url: &str, index: &str, control_index: &str) -> Result<Self> {
        let parsed = url::Url::parse(url).map_err(IndexerError::fatal)?;
        let transport = TransportBuilder::new(SingleNodeConnectionPool::new(parsed))
            .disable_proxy()
            .build()
            .map_err(IndexerError::fatal)?;
        Ok(Self {
            client: OpenSearch::new(transport),
            index: index.to_string(),
            control_index: control_index.to_string(),
        })
    }

    async fn json_of(response: opensearch::http::response::Response, what: &str) -> Result<Value> {
        let status = response.status_code();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let msg = format!("{what} failed with {status}: {body}");
            return Err(if transient_status(status) {
                IndexerError::Transient(msg)
            } else {
                IndexerError::Fatal(msg)
            });
        }
        response.json().await.map_err(IndexerError::transient)
    }

    /// The slot's descriptor as the index registered it, and whether its vector field is mapped.
    pub async fn slot_descriptor(&self, slot: &str) -> Result<(Descriptor, bool)> {
        let response = self
            .client
            .indices()
            .get_mapping(opensearch::indices::IndicesGetMappingParts::Index(&[
                &self.index
            ]))
            .send()
            .await
            .map_err(IndexerError::transient)?;
        let body = Self::json_of(response, "get mapping").await?;
        // An alias resolves to one concrete index; take the first (only) entry.
        let Some((_, m)) = body.as_object().and_then(|o| o.iter().next()) else {
            return Err(IndexerError::Fatal(format!(
                "index {} has no mapping",
                self.index
            )));
        };
        let mappings = &m["mappings"];
        let meta = mappings.get("_meta").cloned().unwrap_or(json!({}));
        let descriptor =
            embedding::slots::slot_descriptor(&meta, slot).map_err(IndexerError::fatal)?;
        let mapped = mappings["properties"]
            .get(descriptor.vector_field())
            .is_some();
        Ok((descriptor, mapped))
    }

    /// One page of hits for a query body.
    pub async fn search(&self, body: Value) -> Result<(Vec<Value>, u64)> {
        let response = self
            .client
            .search(SearchParts::Index(&[&self.index]))
            .body(body)
            .send()
            .await
            .map_err(IndexerError::transient)?;
        let json = Self::json_of(response, "search").await?;
        let hits = json["hits"]["hits"].as_array().cloned().unwrap_or_default();
        Ok((hits, json["took"].as_u64().unwrap_or(0)))
    }

    /// Scripted `update` for each (doc id, body). Missing documents and no-ops are counted, not errors.
    pub async fn bulk_update(&self, ops: &[(String, Value)]) -> Result<BulkOutcome> {
        let mut outcome = BulkOutcome::default();
        if ops.is_empty() {
            return Ok(outcome);
        }
        let started = Instant::now();
        let mut lines: Vec<JsonBody<Value>> = Vec::with_capacity(ops.len() * 2);
        for (id, body) in ops {
            lines.push(JsonBody::new(json!({ "update": { "_id": id } })));
            lines.push(JsonBody::new(body.clone()));
        }
        let response = self
            .client
            .bulk(BulkParts::Index(&self.index))
            .body(lines)
            .send()
            .await
            .map_err(IndexerError::transient)?;
        let json = Self::json_of(response, "bulk").await?;
        outcome.took_ms = started.elapsed().as_millis() as u64;
        for item in json["items"].as_array().into_iter().flatten() {
            let u = &item["update"];
            match (u["status"].as_u64().unwrap_or(0), u["result"].as_str()) {
                (404, _) => outcome.missing += 1,
                (s, Some("noop")) if s < 300 => outcome.noop += 1,
                (s, _) if s < 300 => outcome.updated += 1,
                (s, _) => {
                    let reason = u["error"]["reason"]
                        .as_str()
                        .unwrap_or("unknown")
                        .to_string();
                    if transient_status(
                        opensearch::http::StatusCode::from_u16(s as u16)
                            .unwrap_or(opensearch::http::StatusCode::INTERNAL_SERVER_ERROR),
                    ) {
                        return Err(IndexerError::Transient(format!("bulk item {s}: {reason}")));
                    }
                    outcome.failed.push(format!(
                        "{}: {s} {reason}",
                        u["_id"].as_str().unwrap_or("?")
                    ));
                }
            }
        }
        if !outcome.failed.is_empty() {
            warn!(failed = outcome.failed.len(), sample = ?outcome.failed.first(), "bulk items failed");
        }
        debug!(?outcome, "bulk done");
        Ok(outcome)
    }

    pub async fn ensure_control_index(&self) -> Result<()> {
        let exists = self
            .client
            .indices()
            .exists(opensearch::indices::IndicesExistsParts::Index(&[
                &self.control_index
            ]))
            .send()
            .await
            .map_err(IndexerError::transient)?;
        if exists.status_code().is_success() {
            return Ok(());
        }
        let response = self
            .client
            .indices()
            .create(opensearch::indices::IndicesCreateParts::Index(&self.control_index))
            .body(json!({
                "settings": { "number_of_shards": 1, "number_of_replicas": 1 },
                "mappings": { "dynamic": false, "properties": {
                    "slot": { "type": "keyword" }, "mode": { "type": "keyword" },
                    "checkpoint_ms": { "type": "long" }, "backfill_started_ms": { "type": "long" },
                    "updated_at": { "type": "date" }, "backfill_after": { "type": "object", "enabled": false },
                    "last_cycle": { "type": "object", "enabled": false }
                } }
            }))
            .send()
            .await
            .map_err(IndexerError::transient)?;
        if response.status_code().as_u16() == 400 {
            // Created concurrently by another process: fine.
            return Ok(());
        }
        Self::json_of(response, "create control index")
            .await
            .map(|_| ())
    }

    pub async fn load_control(&self, slot: &str) -> Result<Option<ControlDoc>> {
        let response = self
            .client
            .get(opensearch::GetParts::IndexId(&self.control_index, slot))
            .send()
            .await
            .map_err(IndexerError::transient)?;
        if response.status_code().as_u16() == 404 {
            return Ok(None);
        }
        let json = Self::json_of(response, "get control").await?;
        if json["found"].as_bool() != Some(true) {
            return Ok(None);
        }
        serde_json::from_value(json["_source"].clone())
            .map(Some)
            .map_err(|e| IndexerError::Fatal(format!("control document does not parse: {e}")))
    }

    pub async fn save_control(&self, doc: &ControlDoc) -> Result<()> {
        let response = self
            .client
            .index(opensearch::IndexParts::IndexId(
                &self.control_index,
                &doc.slot,
            ))
            .body(serde_json::to_value(doc).map_err(IndexerError::fatal)?)
            .send()
            .await
            .map_err(IndexerError::transient)?;
        Self::json_of(response, "save control").await.map(|_| ())
    }
}
