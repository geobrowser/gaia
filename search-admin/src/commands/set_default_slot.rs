use anyhow::Result;
use clap::Args;
use serde_json::json;

use crate::commands::index_meta;
use crate::embedding_slots;
use crate::opensearch_client;

/// Make a registered slot the one the API uses when a request names none.
#[derive(Args)]
pub struct SetDefaultSlotCommand {
    /// Index version to modify (default: the index the alias points to)
    #[arg(short, long)]
    version: Option<u32>,

    /// Slot id (must already be registered on the index)
    slot: String,
}

impl SetDefaultSlotCommand {
    pub async fn execute(&self, opensearch_url: &str, index_alias: &str) -> Result<()> {
        let client = opensearch_client::create_client(opensearch_url)?;
        let index = index_meta::resolve_index(&client, index_alias, self.version).await?;
        let mappings = index_meta::get_mappings(&client, &index).await?;
        let meta = embedding_slots::meta_with_default(
            index_meta::meta_of(&mappings).as_ref(),
            &self.slot,
        )?;
        index_meta::put_mapping(&client, &index, json!({ "_meta": meta })).await?;
        println!("✓ {index}: default embedding slot is now {}", self.slot);
        Ok(())
    }
}
