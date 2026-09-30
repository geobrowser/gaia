use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;
use embedding::Descriptor;
use serde_json::{Value, json};
use tracing::info;

use crate::commands::index_meta;
use crate::embedding_slots;
use crate::opensearch_client;

/// Register an embedding slot on an index: add its three k-NN fields (additive `put_mapping`)
/// and file its descriptor under `_meta.embedding_slots.<slot>`. The slot id is derived from the
/// descriptor; nobody types it. Idempotent for an already-registered slot.
#[derive(Args)]
pub struct AddEmbeddingSlotCommand {
    /// Index version to modify (default: the index the alias points to)
    #[arg(short, long)]
    version: Option<u32>,

    /// Descriptor file (a bundle.json)
    #[arg(long, conflicts_with_all = ["from_service", "slot"])]
    spec: Option<PathBuf>,

    /// Read the descriptor from a running embedding-service (its /info), e.g. http://embedding-service:8080
    #[arg(long, requires = "slot")]
    from_service: Option<String>,

    /// Which loaded slot to take from the service
    #[arg(long, requires = "from_service")]
    slot: Option<String>,

    /// Make this slot the index default even if one is already set
    #[arg(long, default_value_t = false)]
    set_default: bool,
}

impl AddEmbeddingSlotCommand {
    async fn descriptor(&self) -> Result<Descriptor> {
        if let Some(spec) = &self.spec {
            let text = std::fs::read_to_string(spec)
                .with_context(|| format!("reading {}", spec.display()))?;
            let d: Descriptor = serde_json::from_str(&text)
                .with_context(|| format!("parsing {}", spec.display()))?;
            d.validate()?;
            return Ok(d);
        }
        let (Some(url), Some(slot)) = (&self.from_service, &self.slot) else {
            bail!("pass --spec <bundle.json>, or --from-service <url> --slot <id>");
        };
        let info: Value = reqwest::Client::new()
            .get(format!("{}/info", url.trim_end_matches('/')))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("parse /info")?;
        let Some(raw) = info["slots"]
            .get(slot.as_str())
            .map(|s| s["descriptor"].clone())
        else {
            bail!("service at {url} has no slot {slot} loaded");
        };
        let d: Descriptor =
            serde_json::from_value(raw).context("service descriptor does not parse")?;
        d.validate()?;
        if d.slot_id() != *slot {
            bail!(
                "service lists slot {slot} but its descriptor hashes to {}",
                d.slot_id()
            );
        }
        Ok(d)
    }

    pub async fn execute(&self, opensearch_url: &str, index_alias: &str) -> Result<()> {
        let d = self.descriptor().await?;
        let slot = d.slot_id();
        let field = d.vector_field();
        let client = opensearch_client::create_client(opensearch_url)?;
        let index = index_meta::resolve_index(&client, index_alias, self.version).await?;

        if !index_meta::knn_enabled(&client, &index).await? {
            bail!(
                "index {index} was created without index.knn, so k-NN fields cannot be queried on it. \
                 Create the next version with the current mapping (create-index) and run full-migration, \
                 then register the slot there."
            );
        }

        let mappings = index_meta::get_mappings(&client, &index).await?;
        if let Some(existing) = mappings["properties"].get(&field) {
            let dims = existing["dimension"].as_u64().unwrap_or(0) as usize;
            if dims != d.dimensions {
                bail!(
                    "{field} already exists on {index} with dimension {dims}, descriptor says {}",
                    d.dimensions
                );
            }
            info!(field = %field, "fields already present; re-registering descriptor in _meta");
        }
        let meta = embedding_slots::meta_with_slot(
            index_meta::meta_of(&mappings).as_ref(),
            &d,
            self.set_default,
        );
        let body = json!({
            "properties": Value::Object(embedding_slots::slot_field_mappings(&d)),
            "_meta": meta,
        });
        index_meta::put_mapping(&client, &index, body).await?;

        let after = index_meta::get_mappings(&client, &index).await?;
        if after["properties"].get(&field).is_none() {
            bail!("mapping update did not take: {field} missing after put_mapping");
        }
        let default = index_meta::meta_of(&after).and_then(|m| embedding_slots::default_slot(&m));

        println!("\n════════════════════════════════════════════════");
        println!("✓ Embedding slot registered");
        println!("════════════════════════════════════════════════");
        println!("Index:        {index}");
        println!("Slot:         {slot}");
        println!("Vector field: {field}");
        println!(
            "Model:        {} ({} d, {})",
            d.model_id, d.dimensions, d.source
        );
        println!("Default slot: {}", default.as_deref().unwrap_or("(none)"));
        println!();
        println!(
            "Next: point an embedding-indexer at slot {slot}; the API serves it once the service has it loaded."
        );
        Ok(())
    }
}
