use anyhow::{Context, Result};
use clap::Args;
use serde_json::json;
use tracing::{info, warn};

use crate::commands::get;
use crate::opensearch_client;

#[derive(Args)]
pub struct UpdateAliasCommand {
    /// Index version to point the alias to
    #[arg(long, short = 'v')]
    version: u32,
}

impl UpdateAliasCommand {
    pub async fn execute(&self, opensearch_url: &str, index_alias: &str) -> Result<()> {
        info!(
            alias = index_alias,
            version = self.version,
            "Updating alias to point to new index version"
        );

        let client = opensearch_client::create_client(opensearch_url)?;

        let versioned_index_name = format!("{}_v{}", index_alias, self.version);

        println!("\n════════════════════════════════════════════════");
        println!("Update Index Alias");
        println!("════════════════════════════════════════════════");
        println!();
        println!("Alias:        {}", index_alias);
        println!("New Index:    {}", versioned_index_name);
        println!();

        // Check if the target index exists
        let index_exists = get::index_exists(&client, &versioned_index_name).await?;

        if !index_exists {
            anyhow::bail!(
                "Index {} does not exist. Create it first with create-index command.",
                versioned_index_name
            );
        }

        println!("✓ Target index exists");
        println!();

        // Get current alias mapping (if any)
        let alias_response = client
            .indices()
            .get_alias(
                opensearch::indices::IndicesGetAliasParts::Name(&[index_alias]),
            )
            .send()
            .await;

        let mut old_index: Option<String> = None;
        if let Ok(response) = alias_response {
            if response.status_code().is_success() {
                let alias_data: serde_json::Value = response
                    .json()
                    .await
                    .context("Failed to parse alias response")?;

                // Get the first index that has this alias
                if let Some(obj) = alias_data.as_object() {
                    old_index = obj.keys().next().map(|s| s.to_string());
                }
            }
        }

        if let Some(ref current_index) = old_index {
            println!("Current alias mapping: {} -> {}", index_alias, current_index);

            if current_index == &versioned_index_name {
                warn!("Alias already points to {}. Nothing to do.", versioned_index_name);
                return Ok(());
            }
        } else {
            println!("Alias {} does not currently exist", index_alias);
        }
        println!();

        // Build the alias update request
        let mut actions = vec![json!({
            "add": {
                "index": versioned_index_name,
                "alias": index_alias
            }
        })];

        // If there's an old index, remove the alias from it
        if let Some(current_index) = old_index {
            actions.insert(
                0,
                json!({
                    "remove": {
                        "index": current_index,
                        "alias": index_alias
                    }
                }),
            );
        }

        let update_body = json!({
            "actions": actions
        });

        println!("Updating alias...");

        // Update the alias
        let update_response = client
            .indices()
            .update_aliases()
            .body(update_body)
            .send()
            .await
            .context("Failed to update alias")?;

        if !update_response.status_code().is_success() {
            let error_body = update_response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to update alias: {}", error_body);
        }

        println!();
        println!("════════════════════════════════════════════════");
        println!("✓ Alias Updated Successfully");
        println!("════════════════════════════════════════════════");
        println!();
        println!(
            "Alias '{}' now points to '{}'",
            index_alias, versioned_index_name
        );
        println!();
        println!("Next steps:");
        println!("1. Start the search-indexer with the new version");
        println!("2. Monitor for any errors");
        println!("3. After confidence period, delete the old index");
        println!();

        Ok(())
    }
}
