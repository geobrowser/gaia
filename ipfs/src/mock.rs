//! Mock IPFS client for testing and local development.
//!
//! The `MockIpfsClient` can be pre-populated with CID → bytes mappings,
//! allowing tests to run without network access.
//!
//! # Example (GRC-20 v2)
//!
//! ```ignore
//! use ipfs::{MockIpfsClient, IpfsFetcher};
//! use grc_20::{encode_edit, Edit, Op, CreateEntity, PropertyValue, Value};
//! use std::borrow::Cow;
//!
//! // Create GRC-20 v2 edit bytes
//! let edit = Edit {
//!     id: [0x01; 16],
//!     name: Cow::Borrowed("Test Edit"),
//!     authors: vec![[0x02; 16]],
//!     created_at: 1700000000,
//!     ops: vec![],
//! };
//! let bytes = encode_edit(&edit).unwrap();
//!
//! let client = MockIpfsClient::new();
//! client.register_bytes("QmTestCid123", bytes);
//! let fetched = client.get_bytes("ipfs://QmTestCid123").await?;
//! ```
//!
//! # Bytes Example
//!
//! ```ignore
//! use ipfs::{MockIpfsClient, IpfsFetcher};
//! use grc_20::{Edit, encode_edit};
//! use std::borrow::Cow;
//!
//! let edit = Edit {
//!     id: [0x01; 16],
//!     name: Cow::Borrowed("Test Edit"),
//!     authors: vec![],
//!     created_at: 1700000000,
//!     ops: vec![],
//! };
//! let bytes = encode_edit(&edit).unwrap();
//!
//! let client = MockIpfsClient::new();
//! client.register_bytes("QmTestCid123", bytes);
//! let fetched = client.get_bytes("ipfs://QmTestCid123").await?;
//! ```

use std::collections::HashMap;
use std::sync::RwLock;

use crate::{IpfsError, IpfsFetcher, Result};
use async_trait::async_trait;

/// Mock IPFS client that returns pre-configured edit data.
///
/// Use this for testing and local development without network access.
pub struct MockIpfsClient {
    /// Map of CID -> serialized Edit bytes
    edits: RwLock<HashMap<String, Vec<u8>>>,
}

impl MockIpfsClient {
    /// Create a new empty mock client.
    pub fn new() -> Self {
        Self {
            edits: RwLock::new(HashMap::new()),
        }
    }

    /// Create a mock client pre-populated with the given CID → bytes mappings.
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
    /// data.insert("QmCid1".to_string(), bytes);
    /// let client = MockIpfsClient::with_bytes(data);
    /// ```
    pub fn with_bytes(data: HashMap<String, Vec<u8>>) -> Self {
        let client = Self::new();
        for (cid, bytes) in data {
            client.register_bytes(&cid, bytes);
        }
        client
    }

    /// Register raw bytes to be returned for a given CID.
    ///
    /// Use this with GRC-20 v2 encoded bytes (`grc_20::encode_edit`).
    /// The CID can be provided with or without the `ipfs://` prefix.
    pub fn register_bytes(&self, cid: &str, bytes: Vec<u8>) {
        let normalized_cid = normalize_cid(cid);
        self.edits.write().unwrap().insert(normalized_cid, bytes);
    }

    /// Check if a CID is registered in the mock.
    pub fn has_cid(&self, cid: &str) -> bool {
        let normalized_cid = normalize_cid(cid);
        self.edits.read().unwrap().contains_key(&normalized_cid)
    }

    /// Get the number of registered edits.
    pub fn len(&self) -> usize {
        self.edits.read().unwrap().len()
    }

    /// Check if the mock is empty.
    pub fn is_empty(&self) -> bool {
        self.edits.read().unwrap().is_empty()
    }
}

impl Default for MockIpfsClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl IpfsFetcher for MockIpfsClient {
    async fn get_bytes(&self, uri: &str) -> Result<Vec<u8>> {
        let cid = normalize_cid(uri);
        self.edits
            .read()
            .unwrap()
            .get(&cid)
            .cloned()
            .ok_or_else(|| IpfsError::NotFound(format!("CID not found in mock: {}", cid)))
    }
}

/// Normalize a CID by removing the `ipfs://` prefix if present.
fn normalize_cid(uri: &str) -> String {
    if let Some((_, cid)) = uri.split_once("://") {
        cid.to_string()
    } else {
        uri.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_client_with_bytes() {
        let mut data = HashMap::new();
        data.insert("QmTestCid1".to_string(), b"Edit 1".to_vec());
        data.insert("QmTestCid2".to_string(), b"Edit 2".to_vec());
        let client = MockIpfsClient::with_bytes(data);

        assert_eq!(client.len(), 2);
        assert!(client.has_cid("QmTestCid1"));
        assert!(client.has_cid("QmTestCid2"));
        assert!(!client.has_cid("QmUnknown"));
    }

    #[tokio::test]
    async fn test_mock_client_get_with_prefix() {
        let client = MockIpfsClient::new();
        client.register_bytes("QmTestCid", b"Test".to_vec());

        // Should work with ipfs:// prefix
        let bytes = client.get_bytes("ipfs://QmTestCid").await.unwrap();
        assert_eq!(bytes, b"Test".to_vec());
    }

    #[tokio::test]
    async fn test_mock_client_get_without_prefix() {
        let client = MockIpfsClient::new();
        client.register_bytes("QmTestCid", b"Test".to_vec());

        // Should work without prefix
        let bytes = client.get_bytes("QmTestCid").await.unwrap();
        assert_eq!(bytes, b"Test".to_vec());
    }

    #[tokio::test]
    async fn test_mock_client_not_found() {
        let client = MockIpfsClient::new();

        let result = client.get_bytes("ipfs://QmUnknown").await;
        assert!(result.is_err());

        if let Err(crate::IpfsError::NotFound(msg)) = result {
            assert!(msg.contains("QmUnknown"));
        } else {
            panic!("Expected NotFound error");
        }
    }

    #[tokio::test]
    async fn test_mock_client_register_with_prefix() {
        let client = MockIpfsClient::new();

        // Register with prefix
        client.register_bytes("ipfs://QmTestCid", b"Test".to_vec());

        // Should still be found without prefix
        assert!(client.has_cid("QmTestCid"));

        // And with prefix
        let bytes = client.get_bytes("ipfs://QmTestCid").await.unwrap();
        assert_eq!(bytes, b"Test".to_vec());
    }
}
