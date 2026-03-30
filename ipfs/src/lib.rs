//! IPFS client for fetching GRC-20 edit content.
//!
//! This crate provides:
//! - [`IpfsSource`] config enum for choosing between mock and live IPFS clients
//! - [`IpfsFetcher`] trait for abstracting IPFS access
//! - [`IpfsClient`] production client that fetches from an IPFS gateway
//! - [`MockIpfsClient`] mock client for testing with pre-configured CID → bytes mappings
//!
//! ## Usage with IpfsSource (Recommended)
//!
//! ```ignore
//! use ipfs::IpfsSource;
//! use std::collections::HashMap;
//!
//! // Development/testing: use mock data
//! let mut data = HashMap::new();
//! data.insert("QmTestCid1".to_string(), grc20_bytes);
//! let fetcher = IpfsSource::mock_bytes(data).into_fetcher();
//!
//! // Production: use live gateway
//! let fetcher = IpfsSource::live("https://ipfs.io/ipfs/").into_fetcher();
//!
//! // Use the fetcher
//! let bytes = fetcher.get_bytes("ipfs://QmTestCid1").await?;
//! ```

mod mock;

pub use mock::MockIpfsClient;

use std::collections::HashMap;

use async_trait::async_trait;
use reqwest::Client as ReqwestClient;

#[derive(Debug, thiserror::Error)]
pub enum IpfsError {
    #[error("reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
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
/// async fn fetch_bytes<F: IpfsFetcher>(fetcher: &F, uri: &str) -> Result<Vec<u8>> {
///     fetcher.get_bytes(uri).await
/// }
/// ```
#[async_trait]
pub trait IpfsFetcher: Send + Sync {
    /// Fetch raw bytes from IPFS by CID.
    ///
    /// The URI should be in the format `ipfs://CID` or just a raw CID.
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
/// let bytes = client.get_bytes("ipfs://QmYwAPJzv5CZsnA...").await?;
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
    async fn get_bytes(&self, uri: &str) -> Result<Vec<u8>> {
        // Strip ipfs:// prefix if present
        let cid = uri.strip_prefix("ipfs://").unwrap_or(uri);

        let url = format!("{}{}", self.url, cid);
        let res = self.client.get(&url).send().await?;

        // Check for HTTP errors before returning bytes
        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(IpfsError::NetworkError(format!(
                "HTTP {}: {}",
                status,
                body.trim()
            )));
        }

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
/// // Development/testing with GRC-20 v2 bytes (recommended)
/// let mut data = HashMap::new();
/// data.insert("QmTestCid1".to_string(), grc20_bytes);
/// let fetcher = IpfsSource::mock_bytes(data).into_fetcher();
///
/// // Production: use live gateway
/// let fetcher = IpfsSource::live("https://ipfs.io/ipfs/").into_fetcher();
/// ```
#[derive(Debug, Clone)]
pub enum IpfsSource {
    /// Use mock IPFS client with pre-configured CID → raw bytes mappings.
    ///
    /// Use this with GRC-20 v2 encoded bytes (`grc_20::encode_edit`).
    /// The map keys are CIDs (with or without `ipfs://` prefix).
    MockBytes(HashMap<String, Vec<u8>>),

    /// Connect to a live IPFS gateway.
    Live {
        /// The IPFS gateway URL (e.g., "https://ipfs.io/ipfs/")
        gateway_url: String,
    },
}

impl IpfsSource {
    /// Create a mock IPFS source with the given CID → raw bytes mappings.
    ///
    /// Use this with GRC-20 v2 encoded bytes (`grc_20::encode_edit`).
    ///
    /// # Example
    ///
    /// ```ignore
    /// use grc_20::{encode_edit, Edit};
    /// use std::borrow::Cow;
    ///
    /// let edit = Edit {
    ///     id: [0x01; 16],
    ///     name: Cow::Borrowed("Test Edit"),
    ///     authors: vec![],
    ///     created_at: 1700000000,
    ///     ops: vec![],
    /// };
    /// let bytes = encode_edit(&edit).unwrap();
    ///
    /// let mut data = HashMap::new();
    /// data.insert("QmTestCid1".to_string(), bytes);
    /// let source = IpfsSource::mock_bytes(data);
    /// ```
    pub fn mock_bytes(data: HashMap<String, Vec<u8>>) -> Self {
        Self::MockBytes(data)
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
            Self::MockBytes(data) => Box::new(MockIpfsClient::with_bytes(data)),
            Self::Live { gateway_url } => Box::new(IpfsClient::new(&gateway_url)),
        }
    }
}
