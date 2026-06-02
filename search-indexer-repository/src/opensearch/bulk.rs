//! Bulk operation utilities for OpenSearch.
//!
//! This module provides functions for executing and parsing bulk operations
//! with OpenSearch.

use bytes::Bytes;
use opensearch::http::request::Body;
use opensearch::params::Refresh;
use opensearch::{BulkOperation, BulkParts, OpenSearch};
use serde::Serialize;
use serde_json::Value;
use tracing::{error, info, warn};

use crate::errors::SearchIndexError;
use crate::opensearch::retry::{self, RetryConfig};
use crate::types::{BatchOperationResult, BatchOperationSummary};

/// Wrapper for bulk update operations with doc_as_upsert support.
#[derive(Serialize)]
pub struct BulkUpdateBody {
    pub doc: Value,
    pub doc_as_upsert: bool,
}

/// Wrapper for bulk scripted update operations.
#[derive(Serialize)]
pub struct BulkScriptBody {
    pub script: BulkScript,
}

/// Script definition for bulk scripted updates.
#[derive(Serialize)]
pub struct BulkScript {
    pub source: String,
    pub lang: &'static str,
}

impl BatchOperationSummary {
    /// Create an empty BatchOperationSummary.
    pub fn empty() -> Self {
        Self {
            total: 0,
            succeeded: 0,
            failed: 0,
            results: Vec::new(),
            wall_ms: 0,
            took_ms: 0,
        }
    }
}

/// Metadata for tracking bulk operation results.
#[derive(Debug, Clone)]
pub struct BulkOperationMeta {
    pub entity_id: String,
    pub space_id: String,
    pub operation_type: String,
}

/// Bulk operation action type supported by OpenSearch bulk API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkAction {
    Create,
    Update,
    Delete,
}

impl BulkAction {
    /// Returns the string representation of the action as used in OpenSearch bulk API responses.
    pub fn as_str(self) -> &'static str {
        match self {
            BulkAction::Create => "create",
            BulkAction::Update => "update",
            BulkAction::Delete => "delete",
        }
    }
}

/// Pre-serialize a `Vec<BulkOperation<B>>` into NDJSON `Bytes` so the body
/// can be re-sent across retry attempts (the original `Vec` is consumed on first use).
fn serialize_bulk_operations<B: Serialize>(
    operations: Vec<BulkOperation<B>>,
) -> Result<Bytes, SearchIndexError> {
    let mut buf = bytes::BytesMut::new();
    for op in &operations {
        Body::write(op, &mut buf).map_err(|e| {
            SearchIndexError::bulk_index(format!("Failed to serialize bulk op: {e}"))
        })?;
        // BulkOperation::write already appends newlines, but guard against missing trailing newline
        if buf.last() != Some(&b'\n') {
            buf.extend_from_slice(b"\n");
        }
    }
    Ok(buf.freeze())
}

/// Execute a bulk request and parse the response into a BatchOperationSummary.
///
/// The operations are pre-serialized to bytes so they can be retried on transient
/// failures (transport errors, 429, 502, 503, 504) with exponential backoff.
///
/// If `refresh` is true, the index will be refreshed after the bulk operation,
/// making all changes immediately visible for search. This is useful when
/// a subsequent operation needs to query for the just-written data.
pub async fn execute_bulk<B: Serialize>(
    client: &OpenSearch,
    alias: &str,
    operations: Vec<BulkOperation<B>>,
    metas: &[BulkOperationMeta],
    action: BulkAction,
    refresh: bool,
    retry_config: &RetryConfig,
) -> Result<BatchOperationSummary, SearchIndexError> {
    let action_str = action.as_str();

    let op_count = metas.len();

    // Pre-serialize so we can retry with the same bytes
    let body_bytes = serialize_bulk_operations(operations)?;
    let payload_bytes = body_bytes.len();

    info!(
        ops = op_count,
        payload_kb = payload_bytes / 1024,
        refresh = refresh,
        "bulk.request.start {} ops → OpenSearch",
        action_str
    );

    let mut attempt = 0u32;
    loop {
        let mut bulk_request = client
            .bulk(BulkParts::Index(alias))
            .body(vec![body_bytes.clone()]);
        if refresh {
            bulk_request = bulk_request.refresh(Refresh::True);
        }

        let start = std::time::Instant::now();
        let result = bulk_request.send().await;
        let wall_ms = start.elapsed().as_millis() as u64;

        let response = match result {
            Ok(resp) => resp,
            Err(e) => {
                // Transport/network error — retryable
                if attempt < retry_config.max_retries {
                    attempt += 1;
                    retry::backoff_sleep(
                        attempt,
                        retry_config,
                        &format!("bulk {action_str} transport error: {e}"),
                    )
                    .await;
                    continue;
                }
                return Err(SearchIndexError::bulk_index(e.to_string()));
            }
        };

        let status = response.status_code();
        if status.is_success() {
            let response_body: Value = response
                .json()
                .await
                .map_err(|e| SearchIndexError::parse(e.to_string()))?;

            let took_ms = response_body
                .get("took")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let mut summary = parse_bulk_response(&response_body, metas, action);
            summary.wall_ms = wall_ms;
            summary.took_ms = took_ms;

            if summary.failed > 0 && attempt < retry_config.max_retries {
                // Item-level failures — retry the whole batch
                attempt += 1;
                retry::backoff_sleep(
                    attempt,
                    retry_config,
                    &format!(
                        "bulk {action_str} {}/{} items failed",
                        summary.failed, summary.total
                    ),
                )
                .await;
                continue;
            }

            if summary.failed > 0 {
                warn!(
                    ops = op_count,
                    succeeded = summary.succeeded,
                    failed = summary.failed,
                    wall_ms = wall_ms,
                    took_ms = took_ms,
                    attempts = attempt + 1,
                    "bulk.request.done {} — completed with failures",
                    action_str
                );
            } else {
                info!(
                    ops = op_count,
                    succeeded = summary.succeeded,
                    wall_ms = wall_ms,
                    took_ms = took_ms,
                    payload_kb = payload_bytes / 1024,
                    "bulk.request.done {} — OK",
                    action_str
                );
            }

            return Ok(summary);
        }

        // Retryable HTTP status (429, 502, 503, 504)
        if retry::is_retryable_status(status.as_u16()) && attempt < retry_config.max_retries {
            let error_body = response.text().await.unwrap_or_default();
            attempt += 1;
            retry::backoff_sleep(
                attempt,
                retry_config,
                &format!("bulk {action_str} HTTP {status}: {error_body}"),
            )
            .await;
            continue;
        }

        // Non-retryable HTTP status or exhausted retries
        let error_body = response.text().await.unwrap_or_default();
        error!(status = %status, body = %error_body, attempt = attempt, "Bulk {} request failed", action_str);
        return Err(SearchIndexError::bulk_index(format!(
            "Bulk {} failed with status {}: {}",
            action_str, status, error_body
        )));
    }
}

/// Parse the bulk API response and build a BatchOperationSummary.
pub fn parse_bulk_response(
    response_body: &Value,
    metas: &[BulkOperationMeta],
    action: BulkAction,
) -> BatchOperationSummary {
    let mut results = Vec::with_capacity(metas.len());
    let mut succeeded = 0;
    let mut failed = 0;

    let items = response_body
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let action_str = action.as_str();

    for (i, meta) in metas.iter().enumerate() {
        let item_result = items.get(i).and_then(|item| item.get(action_str));

        let (success, error) = if let Some(result) = item_result {
            let status = result.get("status").and_then(|s| s.as_u64()).unwrap_or(0);
            // 404 on delete is OK (document not found means it's already deleted).
            // 404 on score updates, space topic updates, topology updates, and
            // relation removals is also OK — the scoring cronjob calculates
            // scores for every entity in Postgres, but we only index entities
            // that have values (name, description, etc.) into OpenSearch. Entities
            // without values won't have a search doc, so 404 is expected and we
            // must not NACK the batch for a doc that legitimately doesn't exist.
            let is_not_found_ok = status == 404
                && (action == BulkAction::Delete
                    || meta.operation_type == "RemoveRelationByDoc"
                    || meta.operation_type == "UpdateEntityGlobalScoreByDoc"
                    || meta.operation_type == "UpdateSpaceScoreByDoc"
                    || meta.operation_type == "UpdateEntitySpaceScore"
                    || meta.operation_type == "UpdateSpaceTopicEntityIdByDoc"
                    || meta.operation_type == "ClearSpaceTopicEntityIdByDoc"
                    || meta.operation_type == "UpdateInCanonicalGraphByDoc");
            let is_success = (200..300).contains(&(status as u16)) || is_not_found_ok;

            if is_success {
                (true, None)
            } else {
                let error_msg = result
                    .get("error")
                    .map(|e| {
                        e.get("reason")
                            .and_then(|r| r.as_str())
                            .map(|reason| reason.to_string())
                            .unwrap_or_else(|| e.to_string())
                    })
                    .unwrap_or_else(|| {
                        format!("Bulk {} failed with status {}", action_str, status)
                    });
                (false, Some(SearchIndexError::bulk_index(error_msg)))
            }
        } else {
            // No result found for this index - this shouldn't happen
            (
                false,
                Some(SearchIndexError::bulk_index(format!(
                    "No result found for operation at index {}",
                    i
                ))),
            )
        };

        if success {
            succeeded += 1;
        } else {
            failed += 1;
        }

        results.push(BatchOperationResult {
            entity_id: meta.entity_id.clone(),
            space_id: meta.space_id.clone(),
            operation_type: meta.operation_type.clone(),
            success,
            error,
        });
    }

    BatchOperationSummary {
        total: metas.len(),
        succeeded,
        failed,
        results,
        wall_ms: 0,
        took_ms: 0,
    }
}
