use anyhow::{Context, Result};
use clap::Args;
use serde_json::json;
use tracing::info;

use search_indexer_repository::opensearch::index_config::get_versioned_index_name;

use crate::opensearch_client;

#[derive(Args)]
pub struct ReindexCommand {
    /// Source index version
    #[arg(short, long)]
    source_version: u32,

    /// Target index version
    #[arg(short, long)]
    target_version: u32,

    /// Wait for reindex to complete (synchronous mode)
    #[arg(long, default_value_t = false)]
    wait_for_completion: bool,

    /// Number of documents to reindex per batch (optional)
    #[arg(long)]
    batch_size: Option<u32>,

    /// Maximum number of documents to reindex (optional, for testing)
    #[arg(long)]
    max_docs: Option<u32>,
}

impl ReindexCommand {
    pub async fn execute(&self, opensearch_url: &str, index_alias: &str) -> Result<()> {
        info!(
            source_version = self.source_version,
            target_version = self.target_version,
            wait_for_completion = self.wait_for_completion,
            "Starting reindex"
        );

        let client = opensearch_client::create_client(opensearch_url)?;

        let source_index = get_versioned_index_name(Some(self.source_version));
        let target_index = get_versioned_index_name(Some(self.target_version));

        println!("\n════════════════════════════════════════════════");
        println!("OpenSearch Reindex");
        println!("════════════════════════════════════════════════");
        println!("Source Index: {}", source_index);
        println!("Target Index: {}", target_index);
        println!(
            "Mode: {}",
            if self.wait_for_completion {
                "Synchronous (wait for completion)"
            } else {
                "Asynchronous (background task)"
            }
        );
        println!();

        // Verify source index exists
        info!("Verifying source index exists...");
        let source_exists = client
            .indices()
            .exists(opensearch::indices::IndicesExistsParts::Index(&[
                &source_index,
            ]))
            .send()
            .await
            .context("Failed to check if source index exists")?
            .status_code()
            .is_success();

        if !source_exists {
            anyhow::bail!(
                "Source index {} does not exist. Cannot reindex.",
                source_index
            );
        }
        info!("✓ Source index exists");

        // Verify target index exists
        info!("Verifying target index exists...");
        let target_exists = client
            .indices()
            .exists(opensearch::indices::IndicesExistsParts::Index(&[
                &target_index,
            ]))
            .send()
            .await
            .context("Failed to check if target index exists")?
            .status_code()
            .is_success();

        if !target_exists {
            anyhow::bail!(
                "Target index {} does not exist. Please create it first using 'create-index' command.",
                target_index
            );
        }
        info!("✓ Target index exists");

        // Get document count from source index
        info!("Getting document count from source index...");
        let count_response = client
            .count(opensearch::CountParts::Index(&[&source_index]))
            .send()
            .await
            .context("Failed to get document count")?;

        let count_json: serde_json::Value = count_response
            .json()
            .await
            .context("Failed to parse count response")?;
        let doc_count = count_json["count"].as_u64().unwrap_or(0);

        println!("Source document count: {}", doc_count);
        println!();

        // Build reindex request
        let mut reindex_body = json!({
            "source": {
                "index": source_index
            },
            "dest": {
                "index": target_index,
                "op_type": "create"  // Fail if document already exists (prevents duplicates)
            }
        });

        // Add optional parameters
        if let Some(size) = self.batch_size {
            reindex_body["source"]["size"] = json!(size);
        }

        if let Some(max) = self.max_docs {
            reindex_body["max_docs"] = json!(max);
        }

        info!(
            wait_for_completion = self.wait_for_completion,
            "Starting reindex operation..."
        );

        let reindex_response = client
            .reindex()
            .wait_for_completion(self.wait_for_completion)
            .body(reindex_body)
            .send()
            .await
            .context("Failed to start reindex operation")?;

        let status = reindex_response.status_code();
        if !status.is_success() {
            let error_body = reindex_response.text().await.unwrap_or_default();
            anyhow::bail!("Reindex failed with status {}: {}", status, error_body);
        }

        let response_json: serde_json::Value = reindex_response
            .json()
            .await
            .context("Failed to parse reindex response")?;

        if self.wait_for_completion {
            // Synchronous mode - reindex completed
            println!("✓ Reindex completed successfully!");
            println!();
            println!("Reindex statistics:");
            println!("  Total: {}", response_json["total"].as_u64().unwrap_or(0));
            println!(
                "  Created: {}",
                response_json["created"].as_u64().unwrap_or(0)
            );
            println!(
                "  Updated: {}",
                response_json["updated"].as_u64().unwrap_or(0)
            );
            println!(
                "  Deleted: {}",
                response_json["deleted"].as_u64().unwrap_or(0)
            );

            if let Some(failures) = response_json["failures"].as_array() {
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
                }
            }

            println!();
            println!("Getting target index document count...");
            let target_count_response = client
                .count(opensearch::CountParts::Index(&[&target_index]))
                .send()
                .await
                .context("Failed to get target document count")?;

            let target_count_json: serde_json::Value = target_count_response
                .json()
                .await
                .context("Failed to parse target count response")?;
            let target_doc_count = target_count_json["count"].as_u64().unwrap_or(0);

            println!("Target document count: {}", target_doc_count);
            println!();

            if target_doc_count == doc_count {
                println!("✓ Document counts match!");
            } else {
                println!(
                    "⚠ Warning: Document counts don't match (source: {}, target: {})",
                    doc_count, target_doc_count
                );
            }
        } else {
            // Asynchronous mode - task started
            let task_id = response_json["task"]
                .as_str()
                .context("Failed to get task ID from response")?;

            println!("✓ Reindex task started successfully!");
            println!();
            println!("Task ID: {}", task_id);
            println!();
            println!("To monitor the reindex progress, run:");
            println!("  search-admin monitor-reindex --task-id {}", task_id);
            println!();
            println!("Or check manually:");
            println!("  curl {}/_tasks/{}", opensearch_url, task_id);
        }

        println!();
        println!("════════════════════════════════════════════════");
        println!("Next steps:");
        println!("════════════════════════════════════════════════");
        if !self.wait_for_completion {
            println!("1. Monitor the reindex task until completion");
        }
        println!(
            "{}. Verify the target index document count matches source",
            if self.wait_for_completion { 1 } else { 2 }
        );
        println!(
            "{}. Update ENTITIES_INDEX_VERSION to {} in search-indexer",
            if self.wait_for_completion { 2 } else { 3 },
            self.target_version
        );
        println!(
            "{}. Restart the search-indexer deployment",
            if self.wait_for_completion { 3 } else { 4 }
        );
        println!(
            "{}. After verification, delete the old index ({})",
            if self.wait_for_completion { 4 } else { 5 },
            source_index
        );

        Ok(())
    }
}
