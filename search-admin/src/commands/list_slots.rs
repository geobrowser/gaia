use anyhow::Result;
use clap::Args;
use serde_json::json;

use crate::commands::index_meta;
use crate::embedding_slots;
use crate::opensearch_client;

/// Show the embedding slots an index carries, which is the default, how many documents each has
/// vectors for, and (optionally) whether a running embedding-service serves them.
#[derive(Args)]
pub struct ListSlotsCommand {
    /// Index version to inspect (default: the index the alias points to)
    #[arg(short, long)]
    version: Option<u32>,

    /// Compare against a running embedding-service, e.g. http://embedding-service:8080
    #[arg(long)]
    embedding_service: Option<String>,
}

impl ListSlotsCommand {
    pub async fn execute(&self, opensearch_url: &str, index_alias: &str) -> Result<()> {
        let client = opensearch_client::create_client(opensearch_url)?;
        let index = index_meta::resolve_index(&client, index_alias, self.version).await?;
        let knn = index_meta::knn_enabled(&client, &index).await?;
        let mappings = index_meta::get_mappings(&client, &index).await?;
        let meta = index_meta::meta_of(&mappings).unwrap_or(json!({}));
        let slots = embedding_slots::slots(&meta)?;
        let default = embedding_slots::default_slot(&meta);

        let total =
            index_meta::count(&client, &index, json!({ "query": { "match_all": {} } })).await?;
        let named = index_meta::count(
            &client,
            &index,
            json!({ "query": { "bool": { "filter": [{ "exists": { "field": "name" } }], "must_not": [{ "term": { "deleted": true } }] } } }),
        )
        .await?;
        let loaded = match &self.embedding_service {
            Some(url) => Some(index_meta::service_loaded_slots(url).await?),
            None => None,
        };

        println!(
            "\nIndex: {index}   index.knn: {knn}   documents: {total}   named & live (embeddable): {named}"
        );
        println!("Default slot: {}", default.as_deref().unwrap_or("(none)"));
        println!(
            "Hybrid pipeline: {}",
            embedding_slots::pipeline_name(index_alias)
        );
        if slots.is_empty() {
            println!("\nNo embedding slots registered. Register one with add-embedding-slot.");
            return Ok(());
        }
        println!();
        for (id, d) in &slots {
            let field = d.vector_field();
            let with_vector = index_meta::count(
                &client,
                &index,
                json!({ "query": { "exists": { "field": &field } } }),
            )
            .await?;
            let coverage = if named > 0 {
                format!("{:.1}%", 100.0 * with_vector as f64 / named as f64)
            } else {
                "—".into()
            };
            let mapped = mappings["properties"].get(&field).is_some();
            let service = match &loaded {
                None => String::new(),
                Some(map) => match map.get(id) {
                    Some(h) if *h == d.hash() => "   service: loaded".to_string(),
                    Some(_) => "   service: HASH MISMATCH".to_string(),
                    None => "   service: NOT LOADED".to_string(),
                },
            };
            println!(
                "{}{id}  {}  {} d  field {field}{}  vectors {with_vector}/{named} ({coverage}){service}",
                if default.as_deref() == Some(id) {
                    "* "
                } else {
                    "  "
                },
                d.model_id,
                d.dimensions,
                if mapped {
                    ""
                } else {
                    " [FIELD MISSING FROM MAPPING]"
                },
            );
            println!(
                "      source {}   floor {}   prompts doc={:?} query={:?}",
                d.source, d.score_floor, d.document_prompt, d.query_prompt
            );
        }
        println!("\n(* = default)");
        Ok(())
    }
}
