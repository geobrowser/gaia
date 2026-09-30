use anyhow::{Context, Result, bail};
use clap::Args;
use opensearch::http::Method;
use opensearch::http::headers::HeaderMap;
use opensearch::http::request::JsonBody;
use serde_json::Value;

use crate::embedding_slots;
use crate::opensearch_client;

/// Create or update the search pipeline hybrid mode sends its requests through. Idempotent.
#[derive(Args)]
pub struct EnsureSearchPipelineCommand {}

impl EnsureSearchPipelineCommand {
    pub async fn execute(&self, opensearch_url: &str, index_alias: &str) -> Result<()> {
        let client = opensearch_client::create_client(opensearch_url)?;
        let name = embedding_slots::pipeline_name(index_alias);
        let path = format!("/_search/pipeline/{name}");
        let response = client
            .transport()
            .send(
                Method::Put,
                &path,
                HeaderMap::new(),
                None::<&()>,
                Some(JsonBody::new(embedding_slots::hybrid_pipeline_body())),
                None,
            )
            .await
            .context("Failed to put search pipeline")?;
        if !response.status_code().is_success() {
            bail!(
                "put search pipeline failed: {}",
                response.text().await.unwrap_or_default()
            );
        }
        let check = client
            .transport()
            .send(
                Method::Get,
                &path,
                HeaderMap::new(),
                None::<&()>,
                None::<JsonBody<Value>>,
                None,
            )
            .await
            .context("Failed to read back search pipeline")?;
        let body: Value = check.json().await.context("parse pipeline")?;
        if body.get(&name).is_none() {
            bail!("pipeline {name} not found after creation: {body}");
        }
        println!("✓ search pipeline {name} in place");
        println!(
            "  {}",
            serde_json::to_string(&body[&name]["phase_results_processors"])?
        );
        Ok(())
    }
}
