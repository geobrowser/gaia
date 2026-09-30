use anyhow::Result;
use clap::Args;
use serde_json::json;

use crate::commands::index_meta;
use crate::embedding_slots;
use crate::opensearch_client;

/// Unregister a slot. Its fields stay in the mapping (OpenSearch cannot drop fields without a
/// reindex); they go away with the next index version. Refuses to retire the default.
#[derive(Args)]
pub struct RetireEmbeddingSlotCommand {
    /// Index version to modify (default: the index the alias points to)
    #[arg(short, long, conflicts_with = "index")]
    version: Option<u32>,

    /// Exact index name to act on (instead of --version or the alias)
    #[arg(long)]
    index: Option<String>,

    /// Slot id to retire
    slot: String,
}

impl RetireEmbeddingSlotCommand {
    pub async fn execute(&self, opensearch_url: &str, index_alias: &str) -> Result<()> {
        let client = opensearch_client::create_client(opensearch_url)?;
        let index =
            index_meta::resolve_index(&client, index_alias, self.version, self.index.as_deref())
                .await?;
        let mappings = index_meta::get_mappings(&client, &index).await?;
        let meta = embedding_slots::meta_without_slot(
            index_meta::meta_of(&mappings).as_ref(),
            &self.slot,
        )?;
        index_meta::put_mapping(&client, &index, json!({ "_meta": meta })).await?;
        println!("✓ {index}: slot {} retired from _meta", self.slot);
        println!(
            "  Its emb_{}* fields remain until the next index version; stop its embedding-indexer.",
            self.slot
        );
        Ok(())
    }
}
