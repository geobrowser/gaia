//! Mock IPFS client for testing and local development.
//!
//! The `MockIpfsClient` can be pre-populated with CID → Edit mappings,
//! allowing tests to run without network access.
//!
//! # Example
//!
//! ```ignore
//! use ipfs::{MockIpfsClient, IpfsFetcher};
//! use wire::pb::grc20::Edit;
//!
//! // Create a mock client with pre-populated edits
//! let edits = vec![
//!     ("QmTestCid123".to_string(), Edit {
//!         id: vec![0x01],
//!         name: "Test Edit".to_string(),
//!         ops: vec![],
//!         authors: vec![],
//!         language: None,
//!     }),
//! ];
//!
//! let client = MockIpfsClient::with_edits(edits);
//! let edit = client.get("ipfs://QmTestCid123").await?;
//! ```

use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;
use prost::Message;
use wire::pb::grc20::Edit;

use crate::{IpfsError, IpfsFetcher, Result};

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

    /// Create a mock client pre-populated with the given CID → Edit mappings.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut edits = HashMap::new();
    /// edits.insert("QmCid1".to_string(), edit1);
    /// edits.insert("QmCid2".to_string(), edit2);
    /// let client = MockIpfsClient::with_edits(edits);
    /// ```
    pub fn with_edits(edits: HashMap<String, Edit>) -> Self {
        let client = Self::new();
        for (cid, edit) in edits {
            client.register_edit(&cid, edit);
        }
        client
    }

    /// Register an edit to be returned for a given CID.
    ///
    /// The CID can be provided with or without the `ipfs://` prefix.
    pub fn register_edit(&self, cid: &str, edit: Edit) {
        let normalized_cid = normalize_cid(cid);
        let bytes = edit.encode_to_vec();
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
    async fn get(&self, uri: &str) -> Result<Edit> {
        let bytes = self.get_bytes(uri).await?;
        let edit = Edit::decode(bytes.as_slice()).map_err(IpfsError::Prost)?;
        Ok(edit)
    }

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

    fn test_edit(name: &str) -> Edit {
        Edit {
            id: vec![0x01, 0x02, 0x03],
            name: name.to_string(),
            ops: vec![],
            authors: vec![],
            language: None,
        }
    }

    #[tokio::test]
    async fn test_mock_client_with_edits() {
        let mut edits = HashMap::new();
        edits.insert("QmTestCid1".to_string(), test_edit("Edit 1"));
        edits.insert("QmTestCid2".to_string(), test_edit("Edit 2"));
        let client = MockIpfsClient::with_edits(edits);

        assert_eq!(client.len(), 2);
        assert!(client.has_cid("QmTestCid1"));
        assert!(client.has_cid("QmTestCid2"));
        assert!(!client.has_cid("QmUnknown"));
    }

    #[tokio::test]
    async fn test_mock_client_get_with_prefix() {
        let client = MockIpfsClient::new();
        client.register_edit("QmTestCid", test_edit("Test"));

        // Should work with ipfs:// prefix
        let edit = client.get("ipfs://QmTestCid").await.unwrap();
        assert_eq!(edit.name, "Test");
    }

    #[tokio::test]
    async fn test_mock_client_get_without_prefix() {
        let client = MockIpfsClient::new();
        client.register_edit("QmTestCid", test_edit("Test"));

        // Should work without prefix
        let edit = client.get("QmTestCid").await.unwrap();
        assert_eq!(edit.name, "Test");
    }

    #[tokio::test]
    async fn test_mock_client_not_found() {
        let client = MockIpfsClient::new();

        let result = client.get("ipfs://QmUnknown").await;
        assert!(result.is_err());

        if let Err(IpfsError::NotFound(msg)) = result {
            assert!(msg.contains("QmUnknown"));
        } else {
            panic!("Expected NotFound error");
        }
    }

    #[tokio::test]
    async fn test_mock_client_register_with_prefix() {
        let client = MockIpfsClient::new();

        // Register with prefix
        client.register_edit("ipfs://QmTestCid", test_edit("Test"));

        // Should still be found without prefix
        assert!(client.has_cid("QmTestCid"));

        // And with prefix
        let edit = client.get("ipfs://QmTestCid").await.unwrap();
        assert_eq!(edit.name, "Test");
    }
}
