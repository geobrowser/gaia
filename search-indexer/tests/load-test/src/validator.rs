use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::config::LoadTestConfig;
use crate::expected_state::{ExpectedRelation, ExpectedState};

const SCORE_EPSILON: f64 = 0.001;

/// Performance stats collected during wait_for_processing.
pub struct ProcessingStats {
    pub duration: Duration,
    /// (doc_count, elapsed_since_start) samples collected every 2s
    pub samples: Vec<(u64, Duration)>,
}

/// Validation results.
pub struct ValidationReport {
    pub total_expected: usize,
    pub passed: usize,
    pub failed: usize,
    pub missing: usize,
    pub extra_docs: usize,
    pub failures: Vec<ValidationFailure>,
    pub duration: Duration,
}

pub struct ValidationFailure {
    pub entity_id: Uuid,
    pub space_id: Uuid,
    pub reason: String,
}

#[derive(Deserialize)]
struct CountResponse {
    count: u64,
}

#[derive(Deserialize)]
struct MgetResponse {
    docs: Vec<MgetDoc>,
}

#[derive(Deserialize)]
struct MgetDoc {
    #[serde(rename = "_id")]
    _id: String,
    found: bool,
    #[serde(rename = "_source")]
    _source: Option<Value>,
}

/// Wait for the indexer to finish processing by polling document count.
pub async fn wait_for_processing(
    client: &reqwest::Client,
    config: &LoadTestConfig,
    expected_count: usize,
) -> Result<ProcessingStats> {
    let url = format!("{}/{}/_count", config.opensearch_url, config.resolved_index());
    let start = Instant::now();
    let deadline = start + Duration::from_secs(config.timeout);
    let mut last_count = 0u64;
    let mut stable_ticks = 0u32;
    let mut samples: Vec<(u64, Duration)> = Vec::new();

    println!(
        "  Waiting for indexer to process (expecting {} docs, timeout {}s)...",
        expected_count, config.timeout
    );

    loop {
        if Instant::now() > deadline {
            println!(
                "  Timeout reached. Last count: {}, expected: {}",
                last_count, expected_count
            );
            break;
        }

        tokio::time::sleep(Duration::from_secs(2)).await;

        let resp = client
            .get(&url)
            .send()
            .await
            .context("Failed to query doc count")?;

        if resp.status().is_success() {
            let count_resp: CountResponse = resp.json().await?;
            let count = count_resp.count;

            samples.push((count, start.elapsed()));

            if count == last_count {
                stable_ticks += 1;
            } else {
                stable_ticks = 0;
            }
            last_count = count;

            print!(
                "\r  Doc count: {} / {} (stable for {}s)    ",
                count,
                expected_count,
                stable_ticks * 2
            );

            // Consider stable if count hasn't changed for 6 seconds
            // and we have at least some documents
            if stable_ticks >= 3 && last_count > 0 {
                println!();
                break;
            }
        }
    }

    let duration = start.elapsed();

    // Refresh index before validation
    let refresh_url = format!("{}/{}/_refresh", config.opensearch_url, config.resolved_index());
    client.post(&refresh_url).send().await?.error_for_status()?;

    Ok(ProcessingStats { duration, samples })
}

/// Validate the OpenSearch state against the expected state.
pub async fn validate(
    client: &reqwest::Client,
    config: &LoadTestConfig,
    expected: &ExpectedState,
) -> Result<ValidationReport> {
    let start = Instant::now();
    let keys: Vec<(Uuid, Uuid)> = expected.documents.keys().cloned().collect();
    let total = keys.len();

    let pb = ProgressBar::new(total as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "  Validating [{bar:40.green/white}] {pos}/{len} ({per_sec}) ETA {eta}",
        )
        .unwrap()
        .progress_chars("=>-"),
    );

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut missing = 0usize;
    let mut failures: Vec<ValidationFailure> = Vec::new();

    // Validate in batches using _mget
    let batch_size = 500;
    for chunk in keys.chunks(batch_size) {
        let docs_request: Vec<Value> = chunk
            .iter()
            .map(|(entity_id, space_id)| {
                // Indexer uses hyphenated UUIDs for doc IDs
                let doc_id = format!(
                    "{}_{}",
                    entity_id.as_hyphenated(),
                    space_id.as_hyphenated()
                );
                serde_json::json!({ "_id": doc_id })
            })
            .collect();

        let mget_url = format!("{}/{}/_mget", config.opensearch_url, config.resolved_index());
        let resp = client
            .post(&mget_url)
            .json(&serde_json::json!({ "docs": docs_request }))
            .send()
            .await
            .context("_mget request failed")?;

        let mget_resp: MgetResponse = resp.json().await.context("Failed to parse _mget response")?;

        for (i, mget_doc) in mget_resp.docs.iter().enumerate() {
            let (entity_id, space_id) = chunk[i];
            let expected_doc = &expected.documents[&(entity_id, space_id)];

            if !mget_doc.found {
                missing += 1;
                if failures.len() < 5000 {
                    failures.push(ValidationFailure {
                        entity_id,
                        space_id,
                        reason: "Document not found in OpenSearch".into(),
                    });
                }
                continue;
            }

            let source = match &mget_doc._source {
                Some(s) => s,
                None => {
                    missing += 1;
                    if failures.len() < 5000 {
                        failures.push(ValidationFailure {
                            entity_id,
                            space_id,
                            reason: "Document found but _source is null".into(),
                        });
                    }
                    continue;
                }
            };

            let mut doc_failures: Vec<String> = Vec::new();

            // Validate name
            let actual_name = source.get("name").and_then(|v| v.as_str());
            match (&expected_doc.name, actual_name) {
                (Some(expected), Some(actual)) if expected != actual => {
                    doc_failures.push(format!(
                        "name: expected {:?}, got {:?}",
                        expected, actual
                    ));
                }
                (Some(expected), None) => {
                    doc_failures.push(format!("name: expected {:?}, got null", expected));
                }
                (None, Some(actual)) if !actual.is_empty() => {
                    doc_failures.push(format!("name: expected null, got {:?}", actual));
                }
                _ => {}
            }

            // Validate description
            let actual_desc = source.get("description").and_then(|v| v.as_str());
            match (&expected_doc.description, actual_desc) {
                (Some(expected), Some(actual)) if expected != actual => {
                    doc_failures.push(format!(
                        "description: expected {:?}, got {:?}",
                        expected, actual
                    ));
                }
                (Some(expected), None) => {
                    doc_failures.push(format!(
                        "description: expected {:?}, got null",
                        expected
                    ));
                }
                (None, Some(actual)) if !actual.is_empty() => {
                    doc_failures.push(format!(
                        "description: expected null, got {:?}",
                        actual
                    ));
                }
                _ => {}
            }

            // Validate image_url
            let actual_image_url = source.get("image_url").and_then(|v| v.as_str());
            match (&expected_doc.image_url, actual_image_url) {
                (Some(expected), Some(actual)) if expected != actual => {
                    doc_failures.push(format!(
                        "image_url: expected {:?}, got {:?}",
                        expected, actual
                    ));
                }
                (Some(expected), None) => {
                    doc_failures.push(format!("image_url: expected {:?}, got null", expected));
                }
                (None, Some(actual)) if !actual.is_empty() => {
                    doc_failures.push(format!("image_url: expected null, got {:?}", actual));
                }
                _ => {}
            }

            // Validate deleted flag
            let actual_deleted = source
                .get("deleted")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if expected_doc.deleted != actual_deleted {
                doc_failures.push(format!(
                    "deleted: expected {}, got {}",
                    expected_doc.deleted, actual_deleted
                ));
            }

            // Validate entity_global_score
            validate_score(
                &mut doc_failures,
                "entity_global_score",
                expected_doc.entity_global_score,
                source.get("entity_global_score").and_then(|v| v.as_f64()),
            );

            // Validate space_score
            validate_score(
                &mut doc_failures,
                "space_score",
                expected_doc.space_score,
                source.get("space_score").and_then(|v| v.as_f64()),
            );

            // Validate entity_space_score
            validate_score(
                &mut doc_failures,
                "entity_space_score",
                expected_doc.entity_space_score,
                source.get("entity_space_score").and_then(|v| v.as_f64()),
            );

            // Validate relations (unordered comparison)
            validate_relations(
                &mut doc_failures,
                &expected_doc.relations,
                source.get("relations"),
            );

            // Validate space_topic_entity_id
            let actual_topic = source
                .get("space_topic_entity_id")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok());
            match (expected_doc.space_topic_entity_id, actual_topic) {
                (Some(expected), Some(actual)) if expected != actual => {
                    doc_failures.push(format!(
                        "space_topic_entity_id: expected {}, got {}",
                        expected, actual
                    ));
                }
                (Some(expected), None) => {
                    doc_failures.push(format!(
                        "space_topic_entity_id: expected {}, got null",
                        expected
                    ));
                }
                _ => {}
            }

            if doc_failures.is_empty() {
                passed += 1;
            } else {
                failed += 1;
                if failures.len() < 5000 {
                    failures.push(ValidationFailure {
                        entity_id,
                        space_id,
                        reason: doc_failures.join("; "),
                    });
                }
            }
        }

        pb.inc(chunk.len() as u64);
    }

    pb.finish_and_clear();

    // Check for extra documents
    let count_url = format!("{}/{}/_count", config.opensearch_url, config.resolved_index());
    let resp = client.get(&count_url).send().await?;
    let count_resp: CountResponse = resp.json().await?;
    let extra_docs = if count_resp.count as usize > total {
        count_resp.count as usize - total
    } else {
        0
    };

    Ok(ValidationReport {
        total_expected: total,
        passed,
        failed,
        missing,
        extra_docs,
        failures,
        duration: start.elapsed(),
    })
}

fn validate_score(
    failures: &mut Vec<String>,
    field: &str,
    expected: Option<f64>,
    actual: Option<f64>,
) {
    match (expected, actual) {
        (Some(e), Some(a)) if (e - a).abs() > SCORE_EPSILON => {
            failures.push(format!("{}: expected {:.6}, got {:.6}", field, e, a));
        }
        (Some(e), None) => {
            failures.push(format!("{}: expected {:.6}, got null", field, e));
        }
        _ => {}
    }
}

fn validate_relations(
    failures: &mut Vec<String>,
    expected: &[ExpectedRelation],
    actual_value: Option<&Value>,
) {
    let actual_rels: Vec<(String, String, String)> = match actual_value {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| {
                let rid = v.get("relation_id")?.as_str()?.to_string();
                let rtype = v.get("relation_type")?.as_str()?.to_string();
                let to_eid = v.get("to_entity_id")?.as_str()?.to_string();
                Some((rid, rtype, to_eid))
            })
            .collect(),
        _ => Vec::new(),
    };

    if expected.len() != actual_rels.len() {
        failures.push(format!(
            "relations count: expected {}, got {}",
            expected.len(),
            actual_rels.len()
        ));
        return;
    }

    // Compare as unordered sets
    let expected_set: HashSet<String> = expected
        .iter()
        .map(|r| {
            format!(
                "{}:{}:{}",
                r.relation_id.as_simple(),
                r.relation_type.as_simple(),
                r.to_entity_id.as_simple()
            )
        })
        .collect();

    let actual_set: HashSet<String> = actual_rels
        .iter()
        .map(|(rid, rtype, to_eid)| {
            // Normalize: strip hyphens for comparison
            let rid_clean = rid.replace('-', "");
            let rtype_clean = rtype.replace('-', "");
            let to_eid_clean = to_eid.replace('-', "");
            format!("{}:{}:{}", rid_clean, rtype_clean, to_eid_clean)
        })
        .collect();

    if expected_set != actual_set {
        let missing: Vec<&String> = expected_set.difference(&actual_set).collect();
        let extra: Vec<&String> = actual_set.difference(&expected_set).collect();
        let mut msg = String::from("relations mismatch:");
        if !missing.is_empty() {
            msg.push_str(&format!(" missing={}", missing.len()));
        }
        if !extra.is_empty() {
            msg.push_str(&format!(" extra={}", extra.len()));
        }
        failures.push(msg);
    }
}

impl ValidationReport {
    pub fn print(&self) {
        println!();
        println!("  === Validation Report ===");
        println!("  Total expected documents: {}", self.total_expected);
        println!("  Passed:  {}", self.passed);
        println!("  Failed:  {}", self.failed);
        println!("  Missing: {}", self.missing);
        println!("  Extra:   {}", self.extra_docs);
        println!("  Duration: {:.1}s", self.duration.as_secs_f64());
        println!();

        if !self.failures.is_empty() {
            // Print failure category breakdown
            let mut score_failures = 0usize;
            let mut relation_failures = 0usize;
            let mut name_failures = 0usize;
            let mut desc_failures = 0usize;
            let mut image_url_failures = 0usize;
            let mut deleted_failures = 0usize;
            let mut topic_failures = 0usize;
            let mut other_failures = 0usize;

            for f in &self.failures {
                let reason = &f.reason;
                if reason.contains("entity_global_score") || reason.contains("space_score") || reason.contains("entity_space_score") {
                    score_failures += 1;
                } else if reason.contains("relations") {
                    relation_failures += 1;
                } else if reason.contains("name:") {
                    name_failures += 1;
                } else if reason.contains("description:") {
                    desc_failures += 1;
                } else if reason.contains("image_url:") {
                    image_url_failures += 1;
                } else if reason.contains("deleted:") {
                    deleted_failures += 1;
                } else if reason.contains("space_topic") {
                    topic_failures += 1;
                } else {
                    other_failures += 1;
                }
            }

            println!("  Failure breakdown:");
            if score_failures > 0 { println!("    Score mismatches:     {}", score_failures); }
            if relation_failures > 0 { println!("    Relation mismatches:  {}", relation_failures); }
            if name_failures > 0 { println!("    Name mismatches:      {}", name_failures); }
            if desc_failures > 0 { println!("    Desc mismatches:      {}", desc_failures); }
            if image_url_failures > 0 { println!("    Image URL mismatches: {}", image_url_failures); }
            if deleted_failures > 0 { println!("    Deleted mismatches:   {}", deleted_failures); }
            if topic_failures > 0 { println!("    Topic mismatches:     {}", topic_failures); }
            if other_failures > 0 { println!("    Other:                {}", other_failures); }
            println!();

            println!(
                "  First {} failures:",
                self.failures.len().min(20)
            );
            for (i, f) in self.failures.iter().take(20).enumerate() {
                println!(
                    "    {}. ({}, {}) {}",
                    i + 1,
                    f.entity_id.as_simple(),
                    f.space_id.as_simple(),
                    f.reason
                );
            }
            if self.failures.len() > 20 {
                println!(
                    "    ... and {} more",
                    self.failures.len() - 20
                );
            }
        }

        if self.is_pass() {
            println!("  RESULT: PASS");
        } else {
            println!("  RESULT: FAIL");
        }
    }

    pub fn is_pass(&self) -> bool {
        // extra_docs is intentionally excluded: interleaved scores may create
        // docs (via doc_as_upsert) not tracked in expected state, and deleted
        // entities remain in the index.
        self.failed == 0 && self.missing == 0
    }
}
