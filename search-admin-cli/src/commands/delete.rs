use anyhow::{Context, Result};
use clap::Args;
use tracing::info;

use search_indexer_repository::opensearch::index_config::get_versioned_index_name;

use crate::opensearch_client;

#[derive(Args)]
pub struct DeleteIndexCommand {
    /// Index version to delete
    #[arg(short, long)]
    version: u32,

    /// Confirm deletion (required safety check)
    #[arg(long, default_value_t = false)]
    confirm: bool,

    /// Skip confirmation prompt (use with --confirm for non-interactive deletion)
    #[arg(long, default_value_t = false)]
    yes: bool,
}

impl DeleteIndexCommand {
    pub async fn execute(&self, opensearch_url: &str, index_alias: &str) -> Result<()> {
        info!(version = self.version, "Deleting index");

        let client = opensearch_client::create_client(opensearch_url)?;
        let versioned_index_name = get_versioned_index_name(Some(self.version));

        println!("\n════════════════════════════════════════════════");
        println!("OpenSearch Delete Index");
        println!("════════════════════════════════════════════════");
        println!("Target Index: {}", versioned_index_name);
        println!();

        // Safety check: require --confirm flag
        if !self.confirm {
            println!("⚠️  SAFETY CHECK FAILED");
            println!();
            println!("This is a destructive operation that will permanently delete:");
            println!("  {}", versioned_index_name);
            println!();
            println!("To proceed, add the --confirm flag:");
            println!("  search-admin delete-index --version {} --confirm", self.version);
            println!();
            anyhow::bail!("Deletion cancelled: --confirm flag required");
        }

        // Verify index exists
        info!("Verifying index exists...");
        let index_exists = client
            .indices()
            .exists(opensearch::indices::IndicesExistsParts::Index(&[
                &versioned_index_name,
            ]))
            .send()
            .await
            .context("Failed to check if index exists")?
            .status_code()
            .is_success();

        if !index_exists {
            anyhow::bail!("Index {} does not exist", versioned_index_name);
        }
        info!("✓ Index exists");
        println!("✓ Index exists");
        println!();

        // Get index statistics
        info!("Getting index statistics...");
        let stats_response = client
            .indices()
            .stats(opensearch::indices::IndicesStatsParts::Index(&[
                &versioned_index_name,
            ]))
            .metric(&["docs", "store"])
            .send()
            .await
            .context("Failed to get index statistics")?;

        if stats_response.status_code().is_success() {
            let stats_json: serde_json::Value = stats_response
                .json()
                .await
                .context("Failed to parse stats response")?;

            if let Some(indices) = stats_json["indices"].as_object() {
                if let Some(index_stats) = indices.get(&versioned_index_name) {
                    let doc_count = index_stats["primaries"]["docs"]["count"]
                        .as_u64()
                        .unwrap_or(0);
                    let store_size = index_stats["primaries"]["store"]["size_in_bytes"]
                        .as_u64()
                        .unwrap_or(0);

                    println!("Index statistics:");
                    println!("  Documents: {}", doc_count);
                    println!(
                        "  Size: {} MB",
                        (store_size as f64 / 1024.0 / 1024.0).round()
                    );
                    println!();
                }
            }
        }

        // Check if index is currently active (pointed to by alias)
        info!("Checking if index is currently active...");
        let alias_response = client
            .indices()
            .get_alias(opensearch::indices::IndicesGetAliasParts::Name(&[
                index_alias,
            ]))
            .send()
            .await;

        if let Ok(response) = alias_response {
            if response.status_code().is_success() {
                let alias_json: serde_json::Value = response
                    .json()
                    .await
                    .context("Failed to parse alias response")?;

                if alias_json
                    .as_object()
                    .and_then(|obj| obj.get(&versioned_index_name))
                    .is_some()
                {
                    println!("❌ ERROR: Index {} is currently ACTIVE!", versioned_index_name);
                    println!("The alias '{}' is pointing to this index.", index_alias);
                    println!();
                    println!("You must switch the alias to a different index before deleting.");
                    println!("Update ENTITIES_INDEX_VERSION and restart search-indexer first.");
                    println!();
                    anyhow::bail!("Cannot delete active index");
                }
            }
        }
        println!("✓ Index is not currently active");
        println!();

        // Interactive confirmation prompt (unless --yes is passed)
        if !self.yes {
            println!("════════════════════════════════════════════════");
            println!("⚠️  FINAL WARNING");
            println!("════════════════════════════════════════════════");
            println!("About to DELETE index: {}", versioned_index_name);
            println!();
            println!("This action is IRREVERSIBLE!");
            println!();
            print!("Type 'DELETE' to confirm: ");
            use std::io::{self, BufRead};
            let stdin = io::stdin();
            let mut line = String::new();
            stdin.lock().read_line(&mut line)?;

            if line.trim() != "DELETE" {
                println!();
                println!("Deletion cancelled.");
                return Ok(());
            }
            println!();
        } else {
            println!("⚠️  Proceeding with deletion (--yes flag provided)");
            println!();
        }

        // Delete the index
        info!("Deleting index...");
        println!("Deleting index...");

        let delete_response = client
            .indices()
            .delete(opensearch::indices::IndicesDeleteParts::Index(&[
                &versioned_index_name,
            ]))
            .send()
            .await
            .context("Failed to delete index")?;

        let status = delete_response.status_code();
        if !status.is_success() {
            let error_body = delete_response.text().await.unwrap_or_default();
            anyhow::bail!("Index deletion failed with status {}: {}", status, error_body);
        }

        info!("✓ Index deleted successfully");
        println!("✓ Index deleted successfully");
        println!();

        // Verify index is gone
        info!("Verifying index deletion...");
        let verify_response = client
            .indices()
            .exists(opensearch::indices::IndicesExistsParts::Index(&[
                &versioned_index_name,
            ]))
            .send()
            .await
            .context("Failed to verify index deletion")?;

        if verify_response.status_code().as_u16() == 404 {
            info!("✓ Index confirmed deleted");
            println!("✓ Index confirmed deleted");
        } else {
            println!("⚠ Warning: Index may still exist");
        }

        println!();
        println!("════════════════════════════════════════════════");
        println!("✓ Index deletion complete!");
        println!("════════════════════════════════════════════════");
        println!();
        println!("Deleted index: {}", versioned_index_name);

        Ok(())
    }
}
