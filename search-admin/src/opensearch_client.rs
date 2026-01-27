use anyhow::{Context, Result};
use opensearch::{
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
    OpenSearch,
};
use url::Url;

/// Create a new OpenSearch client connected to the specified URL.
pub fn create_client(url: &str) -> Result<OpenSearch> {
    let parsed_url = Url::parse(url).context("Failed to parse OpenSearch URL")?;

    let conn_pool = SingleNodeConnectionPool::new(parsed_url);
    let transport = TransportBuilder::new(conn_pool)
        .disable_proxy()
        .build()
        .context("Failed to build OpenSearch transport")?;

    Ok(OpenSearch::new(transport))
}
