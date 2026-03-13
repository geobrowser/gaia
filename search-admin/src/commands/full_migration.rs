use anyhow::{Context, Result};
use clap::Args;
use k8s_openapi::api::apps::v1::StatefulSet;
use kube::{
    Client,
    api::{Api, Patch, PatchParams},
};
use opensearch::OpenSearch;
use serde_json::json;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn};

use search_indexer_repository::opensearch::get_index_settings;

use crate::commands::get;
use crate::opensearch_client;

#[derive(Args)]
pub struct FullMigrationCommand {
    /// Source index version
    #[arg(short, long)]
    source_version: u32,

    /// Target index version
    #[arg(short, long)]
    target_version: u32,

    /// Kubernetes namespace (default: search)
    #[arg(long, default_value = "search")]
    namespace: String,

    /// Search indexer StatefulSet name (default: search-indexer)
    #[arg(long, default_value = "search-indexer")]
    statefulset_name: String,
}

impl FullMigrationCommand {
    pub async fn execute(&self, opensearch_url: &str, index_alias: &str) -> Result<()> {
        // Generate versioned index names from the (possibly prefixed) alias
        let source_index = format!("{}_v{}", index_alias, self.source_version);
        let target_index = format!("{}_v{}", index_alias, self.target_version);

        println!("\n════════════════════════════════════════════════");
        println!(
            "Full Index Migration: v{} → v{}",
            self.source_version, self.target_version
        );
        println!("════════════════════════════════════════════════\n");

        info!(
            source_version = self.source_version,
            target_version = self.target_version,
            index_alias = %index_alias,
            source_index = %source_index,
            target_index = %target_index,
            namespace = %self.namespace,
            statefulset = %self.statefulset_name,
            "Starting full migration"
        );

        println!("Source Index: {}", source_index);
        println!("Target Index: {}", target_index);
        println!();

        // Initialize OpenSearch client (used by all steps)
        let client = opensearch_client::create_client(opensearch_url)?;

        // Pre-flight check: Verify source index exists
        println!("Verifying source index exists...");
        let source_exists = get::index_exists(&client, &source_index).await?;

        if !source_exists {
            anyhow::bail!(
                "Source index '{}' does not exist. Cannot proceed with migration.",
                source_index
            );
        }
        println!("✓ Source index exists\n");

        // Initialize Kubernetes client
        let k8s_client = Client::try_default()
            .await
            .context("Failed to initialize Kubernetes client. Ensure KUBECONFIG is set or running in-cluster.")?;

        let statefulsets: Api<StatefulSet> = Api::namespaced(k8s_client.clone(), &self.namespace);

        // Step 1: Create new index
        self.step_create_index(&client, index_alias).await?;

        // Step 2: Stop search-indexer
        self.step_stop_indexer(&statefulsets).await?;

        // Step 3: Reindex data
        self.step_reindex(&client, index_alias, &source_index, &target_index)
            .await?;

        // Step 4: Update alias
        self.step_update_alias(&client, index_alias).await?;

        // Step 5: Start search-indexer with new version
        self.step_start_indexer(&statefulsets).await?;

        println!("\n════════════════════════════════════════════════");
        println!("✓ Migration Complete!");
        println!("════════════════════════════════════════════════\n");
        println!("Search indexer is now using {}", target_index);
        println!();
        println!("Next steps:");
        println!("  1. Monitor the search-indexer logs for any issues:");
        println!("     kubectl logs -n {} -l app={} -f", self.namespace, self.statefulset_name);
        println!();
        println!("  2. Verify search functionality in your application");
        println!();
        println!("  3. After a few days of stable operation, delete the old index:");
        println!(
            "     Edit delete-index-job.yaml (set INDEX_VERSION={}, CONFIRM_DELETE=true)",
            self.source_version
        );
        println!(
            "     kubectl delete job opensearch-delete-index -n {} 2>/dev/null || true",
            self.namespace
        );
        println!("     kubectl apply -f delete-index-job.yaml");
        println!(
            "     kubectl logs -n {} -f job/opensearch-delete-index",
            self.namespace
        );
        println!();

        Ok(())
    }

    async fn step_create_index(&self, client: &OpenSearch, index_alias: &str) -> Result<()> {
        println!("────────────────────────────────────────────────");
        println!("Step 1/5: Creating New Index");
        println!("────────────────────────────────────────────────\n");

        // Generate versioned index name from the (possibly prefixed) alias
        let versioned_index_name = format!("{}_v{}", index_alias, self.target_version);

        info!(
            version = self.target_version,
            index_alias = %index_alias,
            versioned_index_name = %versioned_index_name,
            "Creating index"
        );

        // Check if index exists
        let index_exists = get::index_exists(client, &versioned_index_name).await?;

        if index_exists {
            info!(
                index = %versioned_index_name,
                "Index already exists, skipping creation"
            );
            println!(
                "✓ Index {} already exists, skipping creation\n",
                versioned_index_name
            );
            return Ok(());
        }

        // Create the index
        let settings = get_index_settings(Some(self.target_version));

        let create_response = client
            .indices()
            .create(opensearch::indices::IndicesCreateParts::Index(
                &versioned_index_name,
            ))
            .body(settings)
            .send()
            .await
            .context("Failed to send create index request")?;

        let status = create_response.status_code();
        if !status.is_success() {
            let error_body = create_response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Index creation failed with status {}: {}",
                status,
                error_body
            );
        }

        info!(index = %versioned_index_name, "Index created successfully");
        println!("✓ Index {} created successfully\n", versioned_index_name);

        Ok(())
    }

    async fn step_stop_indexer(&self, statefulsets: &Api<StatefulSet>) -> Result<()> {
        println!("────────────────────────────────────────────────");
        println!("Step 2/5: Stopping Search Indexer");
        println!("────────────────────────────────────────────────\n");

        info!("Scaling down {} to 0 replicas", self.statefulset_name);

        // Scale to 0 replicas
        let patch = json!({
            "spec": {
                "replicas": 0
            }
        });

        statefulsets
            .patch(
                &self.statefulset_name,
                &PatchParams::default(),
                &Patch::Strategic(patch),
            )
            .await
            .context("Failed to scale down statefulset")?;

        println!("✓ Scaled down {} to 0 replicas", self.statefulset_name);

        // Wait for pods to terminate by polling statefulset status
        info!("Waiting for pods to terminate...");
        println!("  Waiting for pods to terminate...");

        let timeout = Duration::from_secs(120);
        let start = std::time::Instant::now();
        let mut poll_interval = Duration::from_secs(1);
        let max_poll_interval = Duration::from_secs(5);

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!(
                    "Timeout waiting for {} to scale down to 0 replicas after {} seconds",
                    self.statefulset_name,
                    timeout.as_secs()
                );
            }

            // Get current statefulset status
            let sts = statefulsets
                .get(&self.statefulset_name)
                .await
                .context("Failed to get statefulset status")?;

            if let Some(status) = sts.status {
                let ready_replicas = status.ready_replicas.or(status.available_replicas).unwrap_or(0);

                if ready_replicas == 0 {
                    info!("All replicas terminated");
                    break;
                }

                info!(
                    ready_replicas = ready_replicas,
                    elapsed_secs = start.elapsed().as_secs(),
                    "Still waiting for replicas to terminate"
                );
            }

            // Sleep with exponential backoff
            sleep(poll_interval).await;
            poll_interval = std::cmp::min(poll_interval * 2, max_poll_interval);
        }

        println!("✓ Search indexer stopped\n");

        Ok(())
    }

    async fn step_reindex(
        &self,
        client: &OpenSearch,
        _index_alias: &str,
        source_index: &str,
        target_index: &str,
    ) -> Result<()> {
        println!("────────────────────────────────────────────────");
        println!("Step 3/5: Reindexing Data");
        println!("────────────────────────────────────────────────\n");

        info!(
            source = %source_index,
            target = %target_index,
            "Starting reindex"
        );

        // Verify source index exists
        let source_exists = get::index_exists(client, source_index).await?;
        if !source_exists {
            anyhow::bail!("Source index {} does not exist", source_index);
        }

        // Verify target index exists
        let target_exists = get::index_exists(client, target_index).await?;
        if !target_exists {
            anyhow::bail!("Target index {} does not exist", target_index);
        }

        // Get document count from source
        let count_response = client
            .count(opensearch::CountParts::Index(&[source_index]))
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

        // Start reindex (async to get task ID)
        let reindex_body = json!({
            "source": {
                "index": source_index
            },
            "dest": {
                "index": target_index,
                "op_type": "create"
            }
        });

        let reindex_response = client
            .reindex()
            .wait_for_completion(false)
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

        let task_id = response_json["task"]
            .as_str()
            .context("Failed to get task ID from response")?;

        println!("Task ID: {}", task_id);
        println!();
        println!("⏳ Waiting for reindex to complete...");
        println!();

        // Poll the task until completion
        let poll_interval = Duration::from_secs(2);
        loop {
            let task_response = client
                .tasks()
                .get(opensearch::tasks::TasksGetParts::TaskId(task_id))
                .send()
                .await;

            match task_response {
                Ok(response) => {
                    let status = response.status_code();

                    if status.as_u16() == 404 {
                        // Task completed very quickly
                        println!("✓ Reindex completed successfully (task finished quickly)!");
                        println!("  Task ID: {}", task_id);
                        println!();
                        break;
                    }

                    if !status.is_success() {
                        let error_body = response.text().await.unwrap_or_default();
                        anyhow::bail!("Failed to get task status: {} - {}", status, error_body);
                    }

                    let task_json: serde_json::Value = response
                        .json()
                        .await
                        .context("Failed to parse task response")?;

                    let completed = task_json["completed"].as_bool().unwrap_or(false);

                    if completed {
                        if let Some(response_data) = task_json.get("response") {
                            println!("✓ Reindex completed successfully!");
                            println!("  Task ID: {}", task_id);
                            println!();
                            println!("Reindex statistics:");
                            println!("  Total: {}", response_data["total"].as_u64().unwrap_or(0));
                            println!(
                                "  Created: {}",
                                response_data["created"].as_u64().unwrap_or(0)
                            );
                            println!(
                                "  Updated: {}",
                                response_data["updated"].as_u64().unwrap_or(0)
                            );

                            if let Some(failures) = response_data["failures"].as_array() {
                                if !failures.is_empty() {
                                    println!("  Failures: {}", failures.len());
                                    println!();
                                    warn!("Some documents failed to reindex");
                                    println!("⚠ Warning: Some documents failed to reindex:");
                                    for (i, failure) in failures.iter().enumerate().take(5) {
                                        println!("  {}. {}", i + 1, failure);
                                    }
                                    if failures.len() > 5 {
                                        println!("  ... and {} more", failures.len() - 5);
                                    }
                                }
                            }
                        }
                        break;
                    }

                    // Show progress
                    if let Some(status_obj) = task_json.get("task").and_then(|t| t.get("status")) {
                        if let Some(created) = status_obj.get("created").and_then(|v| v.as_u64()) {
                            if let Some(total) = status_obj.get("total").and_then(|v| v.as_u64()) {
                                if total > 0 {
                                    let percentage = (created as f64 / total as f64) * 100.0;
                                    print!(
                                        "\r  Progress: {:.1}% ({}/{})",
                                        percentage, created, total
                                    );
                                    std::io::Write::flush(&mut std::io::stdout()).ok();
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("Error checking task: {}", e);
                }
            }

            sleep(poll_interval).await;
        }

        // Verify document count
        let target_count_response = client
            .count(opensearch::CountParts::Index(&[target_index]))
            .send()
            .await
            .context("Failed to get target document count")?;

        let target_count_json: serde_json::Value = target_count_response
            .json()
            .await
            .context("Failed to parse target count response")?;
        let target_doc_count = target_count_json["count"].as_u64().unwrap_or(0);

        println!();
        println!("Target document count: {}", target_doc_count);
        println!();

        if target_doc_count == doc_count {
            println!("✓ Document counts match!\n");
        } else {
            warn!(
                "Document counts don't match (source: {}, target: {})",
                doc_count, target_doc_count
            );
            println!(
                "⚠ Warning: Document counts don't match (source: {}, target: {})\n",
                doc_count, target_doc_count
            );
        }

        Ok(())
    }

    async fn step_update_alias(&self, client: &OpenSearch, index_alias: &str) -> Result<()> {
        println!("────────────────────────────────────────────────");
        println!("Step 4/5: Updating Alias");
        println!("────────────────────────────────────────────────\n");

        // Generate versioned index name from the (possibly prefixed) alias
        let new_index = format!("{}_v{}", index_alias, self.target_version);

        info!(
            alias = %index_alias,
            version = self.target_version,
            new_index = %new_index,
            "Updating alias"
        );

        // Get current alias target(s)
        let alias_response = client
            .cat()
            .aliases(opensearch::cat::CatAliasesParts::Name(&[index_alias]))
            .format("json")
            .send()
            .await;

        let old_indices: Vec<String> = match alias_response {
            Ok(response) if response.status_code().is_success() => {
                let aliases: Vec<serde_json::Value> = response
                    .json()
                    .await
                    .context("Failed to parse alias response")?;

                aliases
                    .iter()
                    .filter_map(|a| a["index"].as_str().map(String::from))
                    .collect()
            }
            _ => {
                info!("No existing alias found");
                Vec::new()
            }
        };

        // Build actions to remove old aliases and add new one
        let mut actions = Vec::new();

        for old_index in &old_indices {
            actions.push(json!({
                "remove": {
                    "index": old_index,
                    "alias": index_alias
                }
            }));
        }

        actions.push(json!({
            "add": {
                "index": new_index,
                "alias": index_alias
            }
        }));

        let update_body = json!({
            "actions": actions
        });

        let update_response = client
            .indices()
            .update_aliases()
            .body(update_body)
            .send()
            .await
            .context("Failed to update aliases")?;

        if !update_response.status_code().is_success() {
            let error_body = update_response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to update alias: {}", error_body);
        }

        info!("Alias updated successfully");
        println!("✓ Alias '{}' now points to {}\n", index_alias, new_index);

        Ok(())
    }

    async fn step_start_indexer(&self, statefulsets: &Api<StatefulSet>) -> Result<()> {
        println!("────────────────────────────────────────────────");
        println!("Step 5/5: Starting Search Indexer");
        println!("────────────────────────────────────────────────\n");

        info!(
            version = self.target_version,
            "Starting search indexer with new version"
        );

        // Update ENTITIES_INDEX_VERSION and scale to 1 replica
        let patch = json!({
            "spec": {
                "replicas": 1,
                "template": {
                    "spec": {
                        "containers": [{
                            "name": "search-indexer",
                            "env": [{
                                "name": "ENTITIES_INDEX_VERSION",
                                "value": self.target_version.to_string()
                            }]
                        }]
                    }
                }
            }
        });

        statefulsets
            .patch(
                &self.statefulset_name,
                &PatchParams::default(),
                &Patch::Strategic(patch),
            )
            .await
            .context("Failed to update statefulset")?;

        println!("✓ Updated ENTITIES_INDEX_VERSION to {}", self.target_version);
        println!("✓ Scaled up {} to 1 replica", self.statefulset_name);
        println!();
        println!("⏳ Waiting for pod to be ready...");

        let timeout = Duration::from_secs(120);
        let start = std::time::Instant::now();
        let mut poll_interval = Duration::from_secs(2);
        let max_poll_interval = Duration::from_secs(10);

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!(
                    "Timeout waiting for {} to become ready after {} seconds",
                    self.statefulset_name,
                    timeout.as_secs()
                );
            }

            let sts = statefulsets
                .get(&self.statefulset_name)
                .await
                .context("Failed to get statefulset status")?;

            if let Some(status) = sts.status {
                let ready_replicas = status.ready_replicas.unwrap_or(0);
                if ready_replicas >= 1 {
                    info!("Search indexer is ready");
                    break;
                }

                info!(
                    ready_replicas = ready_replicas,
                    elapsed_secs = start.elapsed().as_secs(),
                    "Waiting for pod to become ready"
                );
            }

            sleep(poll_interval).await;
            poll_interval = std::cmp::min(poll_interval * 2, max_poll_interval);
        }

        println!("✓ Search indexer started\n");

        Ok(())
    }
}
