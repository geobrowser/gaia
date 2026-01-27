use anyhow::{Context, Result};
use clap::Args;
use tracing::info;

use crate::opensearch_client;

#[derive(Args)]
pub struct ListIndicesCommand {
    /// Show detailed information
    #[arg(long, default_value_t = false)]
    detailed: bool,

    /// Filter by index pattern (e.g., "entities*")
    #[arg(long)]
    pattern: Option<String>,
}

impl ListIndicesCommand {
    pub async fn execute(&self, opensearch_url: &str, index_alias: &str) -> Result<()> {
        info!(detailed = self.detailed, "Listing indices");

        let client = opensearch_client::create_client(opensearch_url)?;

        println!("\n════════════════════════════════════════════════");
        println!("OpenSearch Indices and Aliases");
        println!("════════════════════════════════════════════════");
        println!();

        // Get all indices matching the pattern
        let default_pattern = format!("{}*", index_alias);
        let pattern = self.pattern.as_deref().unwrap_or(&default_pattern);

        // Get indices with stats
        let cat_response = client
            .cat()
            .indices(opensearch::cat::CatIndicesParts::Index(&[pattern]))
            .v(true)
            .h(&["index", "health", "status", "pri", "rep", "docs.count", "store.size"])
            .send()
            .await
            .context("Failed to list indices")?;

        let indices_text = cat_response
            .text()
            .await
            .context("Failed to parse indices response")?;

        println!("Indices:");
        println!("{}", indices_text);

        // Get alias information
        let alias_response = client
            .indices()
            .get_alias(opensearch::indices::IndicesGetAliasParts::Name(&[
                index_alias,
            ]))
            .send()
            .await
            .context("Failed to send alias query")?;

        let status = alias_response.status_code();

        if status.is_success() {
            // 2xx - parse and display aliases
            let alias_json: serde_json::Value = alias_response
                .json()
                .await
                .context("Failed to parse alias response")?;

            println!();
            println!("Aliases:");
            if let Some(indices) = alias_json.as_object() {
                for (index_name, aliases_obj) in indices {
                    if let Some(aliases) = aliases_obj.get("aliases").and_then(|a| a.as_object())
                    {
                        for alias_name in aliases.keys() {
                            println!("  {} → {}", alias_name, index_name);
                        }
                    }
                }
            } else {
                println!("  No aliases found for pattern '{}'", index_alias);
            }
        } else if status.as_u16() == 404 {
            // 404 - alias not found
            println!();
            println!("Aliases:");
            println!("  No aliases found for pattern '{}'", index_alias);
        } else {
            // Any other non-2xx status - surface the error
            let error_body = alias_response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Failed to get aliases: status {} - {}",
                status,
                error_body
            );
        }

        if self.detailed {
            println!();
            println!("════════════════════════════════════════════════");
            println!("Detailed Index Information");
            println!("════════════════════════════════════════════════");
            println!();

            // Get detailed stats for each index
            let stats_response = client
                .indices()
                .stats(opensearch::indices::IndicesStatsParts::Index(&[pattern]))
                .send()
                .await
                .context("Failed to get index statistics")?;

            if stats_response.status_code().is_success() {
                let stats_json: serde_json::Value = stats_response
                    .json()
                    .await
                    .context("Failed to parse stats response")?;

                if let Some(indices) = stats_json["indices"].as_object() {
                    for (index_name, index_stats) in indices {
                        println!("Index: {}", index_name);

                        let doc_count = index_stats["primaries"]["docs"]["count"]
                            .as_u64()
                            .unwrap_or(0);
                        let deleted = index_stats["primaries"]["docs"]["deleted"]
                            .as_u64()
                            .unwrap_or(0);
                        let store_size = index_stats["primaries"]["store"]["size_in_bytes"]
                            .as_u64()
                            .unwrap_or(0);

                        println!("  Documents:");
                        println!("    Total: {}", doc_count);
                        println!("    Deleted: {}", deleted);
                        println!("  Storage:");
                        println!(
                            "    Primary: {:.2} MB",
                            store_size as f64 / 1024.0 / 1024.0
                        );

                        // Get total (including replicas)
                        let total_store = index_stats["total"]["store"]["size_in_bytes"]
                            .as_u64()
                            .unwrap_or(0);
                        println!(
                            "    Total (with replicas): {:.2} MB",
                            total_store as f64 / 1024.0 / 1024.0
                        );

                        println!();
                    }
                }
            }

            // Get settings for each index
            let settings_response = client
                .indices()
                .get(opensearch::indices::IndicesGetParts::Index(&[pattern]))
                .send()
                .await
                .context("Failed to get index settings")?;

            if settings_response.status_code().is_success() {
                let settings_json: serde_json::Value = settings_response
                    .json()
                    .await
                    .context("Failed to parse settings response")?;

                if let Some(indices) = settings_json.as_object() {
                    for (index_name, index_data) in indices {
                        println!("Index: {}", index_name);

                        if let Some(settings) = index_data
                            .get("settings")
                            .and_then(|s| s.get("index"))
                            .and_then(|i| i.as_object())
                        {
                            println!("  Settings:");

                            if let Some(shards) = settings.get("number_of_shards") {
                                println!("    Shards: {}", shards);
                            }
                            if let Some(replicas) = settings.get("number_of_replicas") {
                                println!("    Replicas: {}", replicas);
                            }
                            if let Some(created) = settings.get("creation_date") {
                                if let Some(timestamp) = created.as_str().and_then(|s| s.parse::<i64>().ok()) {
                                    use chrono::{DateTime, Utc};
                                    let dt = DateTime::<Utc>::from_timestamp(timestamp / 1000, 0);
                                    if let Some(dt) = dt {
                                        println!("    Created: {}", dt.format("%Y-%m-%d %H:%M:%S UTC"));
                                    }
                                }
                            }
                            if let Some(version) = settings.get("version").and_then(|v| v.get("created")) {
                                println!("    OpenSearch Version: {}", version);
                            }
                        }

                        println!();
                    }
                }
            }
        }

        println!("════════════════════════════════════════════════");

        Ok(())
    }
}
