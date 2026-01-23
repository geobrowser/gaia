use anyhow::{Context, Result};
use clap::Args;
use tracing::info;

use search_indexer_repository::opensearch::{
    get_index_settings, get_versioned_index_name,
};

use crate::opensearch_client;

#[derive(Args)]
pub struct CreateIndexCommand {
    /// Index version to create
    #[arg(short, long)]
    version: u32,

    /// Skip if index already exists (default: fail if exists)
    #[arg(long, default_value_t = false)]
    skip_if_exists: bool,
}

impl CreateIndexCommand {
    pub async fn execute(&self, opensearch_url: &str, _index_alias: &str) -> Result<()> {
        info!(
            version = self.version,
            skip_if_exists = self.skip_if_exists,
            "Creating index"
        );

        let client = opensearch_client::create_client(opensearch_url)?;
        let versioned_index_name = get_versioned_index_name(Some(self.version));

        // Check if index exists
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

        if index_exists {
            if self.skip_if_exists {
                info!(
                    index = %versioned_index_name,
                    "Index already exists, skipping"
                );
                return Ok(());
            } else {
                anyhow::bail!(
                    "Index {} already exists. Use --skip-if-exists to skip creation if it exists.",
                    versioned_index_name
                );
            }
        }

        // Create the index
        info!(index = %versioned_index_name, "Creating index");
        let settings = get_index_settings(Some(self.version));

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

        info!(
            index = %versioned_index_name,
            "✓ Index created successfully"
        );

        // Verify the index was created
        let verify_response = client
            .indices()
            .get(opensearch::indices::IndicesGetParts::Index(&[
                &versioned_index_name,
            ]))
            .send()
            .await
            .context("Failed to verify index creation")?;

        if verify_response.status_code().is_success() {
            info!("✓ Index verified successfully");
        }

        println!("\n════════════════════════════════════════════════");
        println!("✓ Index creation complete!");
        println!("════════════════════════════════════════════════");
        println!();
        println!("Index: {}", versioned_index_name);
        println!("Version: {}", self.version);
        println!();
        println!("Next steps:");
        println!("1. Stop the search-indexer deployment");
        println!("2. Run reindex to copy data from the old index");
        println!(
            "3. Update ENTITIES_INDEX_VERSION to {} in search-indexer",
            self.version
        );
        println!("4. Start the search-indexer deployment");
        println!("5. After verification, delete the old index");

        Ok(())
    }
}
