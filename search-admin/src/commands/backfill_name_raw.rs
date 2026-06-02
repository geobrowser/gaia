use anyhow::{Context, Result};
use clap::Args;
use serde_json::json;
use std::time::Duration;
use tokio::time::sleep;
use tracing::info;

use crate::opensearch_client;

#[derive(Args)]
pub struct BackfillNameRawCommand {
    /// Index version to backfill
    #[arg(short, long, default_value_t = 0)]
    version: u32,

    /// Dry run — show what would be updated without making changes
    #[arg(long, default_value_t = false)]
    dry_run: bool,
}

impl BackfillNameRawCommand {
    pub async fn execute(&self, opensearch_url: &str, index_alias: &str) -> Result<()> {
        let index_name = format!("{}_v{}", index_alias, self.version);

        info!(
            index = %index_name,
            dry_run = self.dry_run,
            "Starting name_raw backfill"
        );

        let client = opensearch_client::create_client(opensearch_url)?;

        println!();
        println!("════════════════════════════════════════════════");
        println!("Backfill name_raw field");
        println!("════════════════════════════════════════════════");
        println!("Index: {}", index_name);
        println!("Mode: {}", if self.dry_run { "Dry run" } else { "Live" });
        println!();

        // Verify index exists
        let exists = crate::commands::get::index_exists(&client, &index_name).await?;
        if !exists {
            anyhow::bail!("Index {} does not exist", index_name);
        }

        // First, add the name_raw mapping if it doesn't exist yet
        if !self.dry_run {
            info!("Ensuring name_raw mapping exists...");
            let mapping_body = json!({
                "properties": {
                    "name_raw": {
                        "type": "keyword"
                    }
                }
            });

            let mapping_response = client
                .indices()
                .put_mapping(opensearch::indices::IndicesPutMappingParts::Index(&[
                    &index_name,
                ]))
                .body(mapping_body)
                .send()
                .await
                .context("Failed to update index mapping")?;

            if !mapping_response.status_code().is_success() {
                let error_body = mapping_response.text().await.unwrap_or_default();
                anyhow::bail!("Failed to add name_raw mapping: {}", error_body);
            }
            println!("✓ name_raw mapping ensured");
        }

        // Count documents that need backfilling (have name but no name_raw)
        let count_query = json!({
            "query": {
                "bool": {
                    "must": {
                        "exists": { "field": "name" }
                    },
                    "must_not": {
                        "exists": { "field": "name_raw" }
                    }
                }
            }
        });

        let count_response = client
            .count(opensearch::CountParts::Index(&[&index_name]))
            .body(&count_query)
            .send()
            .await
            .context("Failed to count documents needing backfill")?;

        let count_json: serde_json::Value = count_response
            .json()
            .await
            .context("Failed to parse count response")?;
        let docs_to_update = count_json["count"].as_u64().unwrap_or(0);

        println!("Documents needing backfill: {}", docs_to_update);

        if docs_to_update == 0 {
            println!();
            println!("✓ All documents already have name_raw set. Nothing to do.");
            return Ok(());
        }

        if self.dry_run {
            println!();
            println!(
                "Dry run complete. {} documents would be updated.",
                docs_to_update
            );
            println!("Run without --dry-run to apply changes.");
            return Ok(());
        }

        // Run update_by_query to copy name → name_raw for all docs missing name_raw
        println!();
        println!("Running update_by_query to copy name → name_raw...");

        let update_body = json!({
            "script": {
                "source": "ctx._source.name_raw = ctx._source.name",
                "lang": "painless"
            },
            "query": {
                "bool": {
                    "must": {
                        "exists": { "field": "name" }
                    },
                    "must_not": {
                        "exists": { "field": "name_raw" }
                    }
                }
            }
        });

        // Start as async task to get a task ID for progress monitoring
        let response = client
            .update_by_query(opensearch::UpdateByQueryParts::Index(&[&index_name]))
            .wait_for_completion(false)
            .body(update_body)
            .send()
            .await
            .context("Failed to start update_by_query")?;

        let status = response.status_code();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "update_by_query failed with status {}: {}",
                status,
                error_body
            );
        }

        let response_json: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse update_by_query response")?;

        let task_id = response_json["task"]
            .as_str()
            .context("Failed to get task ID from response")?;

        println!("Task ID: {}", task_id);
        println!();
        println!("⏳ Waiting for backfill to complete...");

        // Poll task until completion
        let poll_interval = Duration::from_secs(2);
        loop {
            let task_response = client
                .tasks()
                .get(opensearch::tasks::TasksGetParts::TaskId(task_id))
                .send()
                .await;

            match task_response {
                Ok(resp) => {
                    let resp_status = resp.status_code();

                    if resp_status.as_u16() == 404 {
                        println!("✓ Backfill completed (task finished quickly)");
                        break;
                    }

                    if !resp_status.is_success() {
                        let error_body = resp.text().await.unwrap_or_default();
                        anyhow::bail!(
                            "Failed to get task status: {} - {}",
                            resp_status,
                            error_body
                        );
                    }

                    let task_json: serde_json::Value =
                        resp.json().await.context("Failed to parse task response")?;

                    let completed = task_json["completed"].as_bool().unwrap_or(false);

                    if completed {
                        if let Some(response_data) = task_json.get("response") {
                            let updated = response_data["updated"].as_u64().unwrap_or(0);
                            let total = response_data["total"].as_u64().unwrap_or(0);
                            let failures = response_data["failures"]
                                .as_array()
                                .map(|f| f.len())
                                .unwrap_or(0);

                            println!();
                            println!("✓ Backfill completed!");
                            println!("  Total processed: {}", total);
                            println!("  Updated: {}", updated);

                            if failures > 0 {
                                println!("  Failures: {}", failures);
                                if let Some(failure_list) = response_data["failures"].as_array() {
                                    for (i, failure) in failure_list.iter().enumerate().take(5) {
                                        println!("  {}. {}", i + 1, failure);
                                    }
                                    if failure_list.len() > 5 {
                                        println!("  ... and {} more", failure_list.len() - 5);
                                    }
                                }
                            }
                        }
                        break;
                    }

                    // Show progress
                    if let Some(task_status) = task_json.get("task").and_then(|t| t.get("status")) {
                        if let (Some(updated), Some(total)) = (
                            task_status.get("updated").and_then(|v| v.as_u64()),
                            task_status.get("total").and_then(|v| v.as_u64()),
                        ) {
                            if total > 0 {
                                let pct = (updated as f64 / total as f64) * 100.0;
                                print!("\r  Progress: {:.1}% ({}/{})", pct, updated, total);
                                std::io::Write::flush(&mut std::io::stdout()).ok();
                            }
                        }
                    }
                }
                Err(e) => {
                    println!("⚠ Error checking task: {}", e);
                }
            }

            sleep(poll_interval).await;
        }

        // Verify: count remaining docs without name_raw
        println!();
        println!("Verifying backfill...");

        let verify_response = client
            .count(opensearch::CountParts::Index(&[&index_name]))
            .body(&count_query)
            .send()
            .await
            .context("Failed to verify backfill")?;

        let verify_json: serde_json::Value = verify_response
            .json()
            .await
            .context("Failed to parse verify response")?;
        let remaining = verify_json["count"].as_u64().unwrap_or(0);

        if remaining == 0 {
            println!("✓ All documents now have name_raw set");
        } else {
            println!("⚠ {} documents still missing name_raw", remaining);
        }

        Ok(())
    }
}
