use anyhow::{Context, Result};
use clap::Args;
use std::time::Duration;
use tokio::time::sleep;
use tracing::info;

use crate::opensearch_client;

#[derive(Args)]
pub struct MonitorReindexCommand {
    /// Task ID to monitor
    #[arg(short, long)]
    task_id: String,

    /// Poll interval in seconds
    #[arg(long, default_value_t = 10)]
    poll_interval: u64,

    /// Maximum wait time in seconds (0 for unlimited)
    #[arg(long, default_value_t = 3600)]
    max_wait: u64,

    /// Wait for task to complete before returning
    #[arg(long, default_value_t = false)]
    wait: bool,
}

impl MonitorReindexCommand {
    pub async fn execute(&self, opensearch_url: &str) -> Result<()> {
        info!(
            task_id = %self.task_id,
            poll_interval = self.poll_interval,
            max_wait = self.max_wait,
            "Monitoring reindex task"
        );

        let client = opensearch_client::create_client(opensearch_url)?;

        println!("\n════════════════════════════════════════════════");
        println!("OpenSearch Reindex Task Monitor");
        println!("════════════════════════════════════════════════");
        println!("Task ID: {}", self.task_id);
        println!("Poll interval: {}s", self.poll_interval);
        if self.max_wait > 0 {
            println!("Max wait time: {}s", self.max_wait);
        } else {
            println!("Max wait time: unlimited");
        }
        println!();

        if self.wait {
            // Wait for completion using OpenSearch API
            info!("Waiting for task to complete...");
            let task_response = client
                .tasks()
                .get(opensearch::tasks::TasksGetParts::TaskId(&self.task_id))
                .wait_for_completion(true)
                .timeout(format!("{}s", self.max_wait))
                .send()
                .await
                .context("Failed to wait for task completion")?;

            let status = task_response.status_code();
            if status.as_u16() == 404 {
                println!("Task not found - it may have already completed.");
                return self.check_completed_task(&client).await;
            }

            if !status.is_success() {
                let error_body = task_response.text().await.unwrap_or_default();
                anyhow::bail!("Failed to get task status: {} - {}", status, error_body);
            }

            let task_json: serde_json::Value = task_response
                .json()
                .await
                .context("Failed to parse task response")?;

            self.print_task_result(&task_json)?;
        } else {
            // Poll the task status
            let mut elapsed = 0u64;
            loop {
                info!(elapsed, "Checking task status...");
                println!("Checking task status... (elapsed: {}s)", elapsed);

                let task_response = client
                    .tasks()
                    .get(opensearch::tasks::TasksGetParts::TaskId(&self.task_id))
                    .send()
                    .await;

                match task_response {
                    Ok(response) => {
                        let status = response.status_code();

                        if status.as_u16() == 404 {
                            println!("Task not found - checking completed tasks...");
                            return self.check_completed_task(&client).await;
                        }

                        if !status.is_success() {
                            let error_body = response.text().await.unwrap_or_default();
                            anyhow::bail!(
                                "Failed to get task status: {} - {}",
                                status,
                                error_body
                            );
                        }

                        let task_json: serde_json::Value = response
                            .json()
                            .await
                            .context("Failed to parse task response")?;

                        let completed = task_json["completed"].as_bool().unwrap_or(false);

                        if completed {
                            println!("✓ Task completed!");
                            println!();
                            return self.print_task_result(&task_json);
                        }

                        // Show progress
                        self.print_progress(&task_json);
                    }
                    Err(e) => {
                        println!("⚠ Error checking task: {}", e);
                    }
                }

                if self.max_wait > 0 && elapsed >= self.max_wait {
                    println!();
                    println!("⚠ Maximum wait time reached ({}s)", self.max_wait);
                    println!("Task may still be running. Check manually:");
                    println!("  curl {}/_tasks/{}", opensearch_url, self.task_id);
                    anyhow::bail!("Timeout waiting for task completion");
                }

                sleep(Duration::from_secs(self.poll_interval)).await;
                elapsed += self.poll_interval;
            }
        }

        Ok(())
    }

    fn print_progress(&self, task_json: &serde_json::Value) {
        println!("Task in progress...");

        if let Some(status) = task_json.get("task").and_then(|t| t.get("status")) {
            if let Some(total) = status.get("total").and_then(|v| v.as_u64()) {
                println!("  Total: {}", total);
            }
            if let Some(created) = status.get("created").and_then(|v| v.as_u64()) {
                println!("  Created: {}", created);
            }
            if let Some(updated) = status.get("updated").and_then(|v| v.as_u64()) {
                println!("  Updated: {}", updated);
            }
            if let Some(deleted) = status.get("deleted").and_then(|v| v.as_u64()) {
                println!("  Deleted: {}", deleted);
            }

            // Calculate progress percentage
            if let (Some(created), Some(total)) = (
                status.get("created").and_then(|v| v.as_u64()),
                status.get("total").and_then(|v| v.as_u64()),
            ) {
                if total > 0 {
                    let percentage = (created as f64 / total as f64) * 100.0;
                    println!("  Progress: {:.1}%", percentage);
                }
            }
        }

        println!();
    }

    fn print_task_result(&self, task_json: &serde_json::Value) -> Result<()> {
        println!("Final task status:");
        println!("{}", serde_json::to_string_pretty(task_json)?);
        println!();

        if let Some(response) = task_json.get("response") {
            println!("Reindex statistics:");
            println!("  Total: {}", response["total"].as_u64().unwrap_or(0));
            println!("  Created: {}", response["created"].as_u64().unwrap_or(0));
            println!("  Updated: {}", response["updated"].as_u64().unwrap_or(0));
            println!("  Deleted: {}", response["deleted"].as_u64().unwrap_or(0));
            println!("  Batches: {}", response["batches"].as_u64().unwrap_or(0));

            if let Some(failures) = response["failures"].as_array() {
                if !failures.is_empty() {
                    println!("  Failures: {}", failures.len());
                    println!();
                    println!("⚠ Warning: Some documents failed to reindex:");
                    for (i, failure) in failures.iter().enumerate().take(5) {
                        println!("  {}. {}", i + 1, failure);
                    }
                    if failures.len() > 5 {
                        println!("  ... and {} more", failures.len() - 5);
                    }
                } else {
                    println!("  Failures: 0");
                }
            }
        }

        println!();
        println!("════════════════════════════════════════════════");
        println!("✓ Reindex monitoring complete!");
        println!("════════════════════════════════════════════════");

        Ok(())
    }

    async fn check_completed_task(&self, client: &opensearch::OpenSearch) -> Result<()> {
        println!("Searching for task in completed tasks...");

        // Search for the task in the .tasks index
        let search_response = client
            .search(opensearch::SearchParts::Index(&[".tasks"]))
            .body(serde_json::json!({
                "query": {
                    "term": {
                        "_id": self.task_id
                    }
                }
            }))
            .send()
            .await
            .context("Failed to search completed tasks")?;

        let search_json: serde_json::Value = search_response
            .json()
            .await
            .context("Failed to parse search response")?;

        let hits = search_json["hits"]["hits"]
            .as_array()
            .context("Failed to get search hits")?;

        if hits.is_empty() {
            anyhow::bail!("Task not found in active or completed tasks");
        }

        println!("✓ Task found in completed tasks!");
        println!("{}", serde_json::to_string_pretty(&hits[0])?);

        Ok(())
    }
}
