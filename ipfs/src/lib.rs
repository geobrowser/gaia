//! IPFS client for fetching GRC-20 edit content.
//!
//! This crate provides:
//! - [`IpfsSource`] config enum for choosing between mock and live IPFS clients
//! - [`IpfsFetcher`] trait for abstracting IPFS access
//! - [`IpfsClient`] production client that fetches from an IPFS gateway
//! - [`MockIpfsClient`] mock client for testing with pre-configured CID → Edit mappings
//!
//! ## Usage with IpfsSource (Recommended)
//!
//! ```ignore
//! use ipfs::IpfsSource;
//! use std::collections::HashMap;
//!
//! // Development/testing: use mock data
//! let mut edits = HashMap::new();
//! edits.insert("QmTestCid1".to_string(), edit1);
//! let fetcher = IpfsSource::mock(edits).into_fetcher();
//!
//! // Production: use live gateway
//! let fetcher = IpfsSource::live("https://ipfs.io/ipfs/").into_fetcher();
//!
//! // Use the fetcher
//! let edit = fetcher.get("ipfs://QmTestCid1").await?;
//! ```

mod mock;

pub use mock::MockIpfsClient;

use std::collections::HashMap;

use async_trait::async_trait;
use reqwest::Client as ReqwestClient;
use wire::{
    deserialize::{deserialize, DeserializeError},
    pb::grc20::Edit,
};

#[derive(Debug, thiserror::Error)]
pub enum IpfsError {
    #[error("reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("prost error: {0}")]
    Prost(#[from] prost::DecodeError),
    #[error("cid error: {0}")]
    CidError(String),
    #[error("deserialize error: {0}")]
    DeserializeError(#[from] DeserializeError),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("network error: {0}")]
    NetworkError(String),
    #[error("timeout")]
    Timeout,
}

pub type Result<T> = std::result::Result<T, IpfsError>;

/// Trait for fetching content from IPFS.
///
/// This trait abstracts the IPFS client to enable dependency injection
/// and mocking for testing. Production code uses [`IpfsClient`], while
/// tests can use mock implementations.
///
/// # Example
///
/// ```ignore
/// use ipfs::IpfsFetcher;
///
/// async fn fetch_edit<F: IpfsFetcher>(fetcher: &F, uri: &str) -> Result<Edit> {
///     fetcher.get(uri).await
/// }
/// ```
#[async_trait]
pub trait IpfsFetcher: Send + Sync {
    /// Fetch and decode a GRC-20 Edit from IPFS by URI.
    ///
    /// The URI should be in the format `ipfs://CID` or just a raw CID.
    async fn get(&self, uri: &str) -> Result<Edit>;

    /// Fetch raw bytes from IPFS by CID.
    async fn get_bytes(&self, cid: &str) -> Result<Vec<u8>>;
}

/// Production IPFS client that fetches from a gateway.
///
/// # Example
///
/// ```ignore
/// use ipfs::IpfsClient;
///
/// let client = IpfsClient::new("https://ipfs.io/ipfs/");
/// let edit = client.get("ipfs://QmYwAPJzv5CZsnA...").await?;
/// ```
pub struct IpfsClient {
    url: String,
    client: ReqwestClient,
}

impl IpfsClient {
    pub fn new(url: &str) -> Self {
        IpfsClient {
            url: url.to_string(),
            client: ReqwestClient::new(),
        }
    }
}

#[async_trait]
impl IpfsFetcher for IpfsClient {
    async fn get(&self, hash: &str) -> Result<Edit> {
        // @TODO: Error handle
        let cid = if let Some((_, maybe_cid)) = hash.split_once("://") {
            maybe_cid
        } else {
            ""
        };

        // @TODO: Should retry this fetch
        let bytes = self.get_bytes(cid).await?;

        let data = deserialize(&bytes)?;
        Ok(data)
    }

    async fn get_bytes(&self, hash: &str) -> Result<Vec<u8>> {
        let url = format!("{}{}", self.url, hash);
        let res = self.client.get(&url).send().await?;
        let bytes = res.bytes().await?;
        Ok(bytes.to_vec())
    }
}

/// Configuration for the IPFS data source.
///
/// Use this to explicitly choose between mock and live IPFS clients,
/// following the same pattern as `StreamSource` in hermes-relay.
///
/// # Example
///
/// ```ignore
/// use ipfs::IpfsSource;
/// use std::collections::HashMap;
///
/// // Development/testing: use mock data
/// let mut edits = HashMap::new();
/// edits.insert("QmTestCid1".to_string(), edit1);
/// let fetcher = IpfsSource::mock(edits).into_fetcher();
///
/// // Production: use live gateway
/// let fetcher = IpfsSource::live("https://ipfs.io/ipfs/").into_fetcher();
/// ```
#[derive(Debug, Clone)]
pub enum IpfsSource {
    /// Use mock IPFS client with pre-configured CID → Edit mappings.
    ///
    /// The map keys are CIDs (with or without `ipfs://` prefix).
    Mock(HashMap<String, Edit>),

    /// Connect to a live IPFS gateway.
    Live {
        /// The IPFS gateway URL (e.g., "https://ipfs.io/ipfs/")
        gateway_url: String,
    },
}

impl IpfsSource {
    /// Create a mock IPFS source with the given CID → Edit mappings.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut edits = HashMap::new();
    /// edits.insert("QmTestCid1".to_string(), edit1);
    /// edits.insert("QmTestCid2".to_string(), edit2);
    /// let source = IpfsSource::mock(edits);
    /// ```
    pub fn mock(edits: HashMap<String, Edit>) -> Self {
        Self::Mock(edits)
    }

    /// Create a live IPFS source with the given gateway URL.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let source = IpfsSource::live("https://ipfs.io/ipfs/");
    /// ```
    pub fn live(gateway_url: impl Into<String>) -> Self {
        Self::Live {
            gateway_url: gateway_url.into(),
        }
    }

    /// Create the appropriate IpfsFetcher implementation.
    ///
    /// Returns a boxed trait object that can be used to fetch IPFS content.
    pub fn into_fetcher(self) -> Box<dyn IpfsFetcher> {
        match self {
            Self::Mock(edits) => Box::new(MockIpfsClient::with_edits(edits)),
            Self::Live { gateway_url } => Box::new(IpfsClient::new(&gateway_url)),
        }
    }
}
