//! Blob store for the two Postgres blob tables (`ipfs_cache`, `atlas_checkpoints`).
//!
//! The target is one S3-compatible bucket. Everything here is flag-off by default: a
//! [`Mirror`] is only built when its write-through or shadow-read flag is set, and a failure
//! in the blob store never fails the Postgres path (the Postgres table stays the source until
//! the shadow comparison has been clean for long enough).
//!
//! - [`BlobStore`]: put / get / exists over opaque keys.
//! - [`FsBlobStore`]: filesystem backend (tests, local runs). Atomic replace via rename.
//! - [`S3BlobStore`]: S3-compatible backend (SigV4 over HTTPS, path-style). PUT replaces
//!   atomically.
//! - [`object_key`]: the injective, S3-safe key escape.
//! - [`Mirror`]: the flag-gated write-through and shadow read with metrics.

mod fs;
mod mirror;
mod s3;
mod sigv4;

pub mod ipfs_object;
pub mod json;

pub use fs::FsBlobStore;
pub use mirror::{Mirror, MirrorFlags, ShadowOutcome, digest};
pub use s3::{S3BlobStore, S3Config};

use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum BlobError {
    #[error("io: {0}")]
    Io(String),
    #[error("http: {0}")]
    Http(String),
    #[error("configuration: {0}")]
    Config(String),
    #[error("corrupt object: {0}")]
    Corrupt(String),
}

pub type BlobResult<T> = Result<T, BlobError>;

/// Opaque-key blob storage. Keys must come from [`object_key`].
#[async_trait::async_trait]
pub trait BlobStore: Send + Sync {
    /// Store `bytes` under `key`, atomically replacing any existing object.
    async fn put(&self, key: &str, bytes: &[u8]) -> BlobResult<()>;
    /// Fetch the object, or `None` if it does not exist.
    async fn get(&self, key: &str) -> BlobResult<Option<Vec<u8>>>;
    /// Whether the object exists.
    async fn exists(&self, key: &str) -> BlobResult<bool>;
}

/// Longest escaped key we keep verbatim; S3 allows 1024 bytes, leave room for the prefix.
const MAX_ESCAPED_LEN: usize = 800;

/// Build the object key `<prefix>/<escaped raw>`.
///
/// Bytes outside `[A-Za-z0-9._-]` become `~XX` (uppercase hex), so the mapping is injective
/// and the key is already URL-path-safe (`~` is an unreserved URI character and is itself
/// escaped as `~7E`). A raw value whose escape would exceed 800 bytes is replaced by
/// `~sha256-<hex>`; an escaped value never has `~` followed by anything but two hex digits, so
/// the hashed form cannot collide with an escaped one.
pub fn object_key(prefix: &str, raw: &str) -> String {
    let mut out = String::with_capacity(prefix.len() + 1 + raw.len());
    out.push_str(prefix);
    out.push('/');
    let start = out.len();
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' => out.push(b as char),
            _ => out.push_str(&format!("~{b:02X}")),
        }
    }
    if out.len() - start > MAX_ESCAPED_LEN {
        out.truncate(start);
        out.push_str("~sha256-");
        out.push_str(&digest(raw.as_bytes()).hash);
    }
    out
}

/// Build the store selected by `BLOB_BACKEND` (`s3` or `fs`). Returns `Ok(None)` when unset.
///
/// `s3` reads `BLOB_S3_ENDPOINT`, `BLOB_S3_BUCKET`, `BLOB_S3_ACCESS_KEY`, `BLOB_S3_SECRET_KEY`
/// and optional `BLOB_S3_REGION` (default `us-east-1`). `fs` reads `BLOB_FS_ROOT`.
pub fn store_from_env() -> BlobResult<Option<Arc<dyn BlobStore>>> {
    let backend = match std::env::var("BLOB_BACKEND") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return Ok(None),
    };
    match backend.trim() {
        "s3" => Ok(Some(Arc::new(S3BlobStore::new(S3Config::from_env()?)?))),
        "fs" => {
            let root = std::env::var("BLOB_FS_ROOT")
                .map_err(|_| BlobError::Config("BLOB_FS_ROOT is required for fs".into()))?;
            Ok(Some(Arc::new(FsBlobStore::new(root))))
        }
        other => Err(BlobError::Config(format!("unknown BLOB_BACKEND {other:?}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_escape_is_safe_and_injective() {
        let a = object_key("ipfs_cache", "ipfs://Qm/../x y");
        assert_eq!(a, "ipfs_cache/ipfs~3A~2F~2FQm~2F..~2Fx~20y");
        assert_ne!(object_key("p", "a/b"), object_key("p", "a~2Fb"));
        assert_eq!(object_key("p", "a~2Fb"), "p/a~7E2Fb");
        assert!(!a.contains("//"));
    }

    #[test]
    fn long_values_are_hashed() {
        let raw = "x".repeat(2000);
        let k = object_key("ipfs_cache", &raw);
        assert!(k.starts_with("ipfs_cache/~sha256-"));
        assert!(k.len() < 100);
        assert_eq!(k, object_key("ipfs_cache", &raw));
    }

    #[test]
    fn non_ascii_is_escaped_bytewise() {
        assert_eq!(object_key("p", "é"), "p/~C3~A9");
    }
}
