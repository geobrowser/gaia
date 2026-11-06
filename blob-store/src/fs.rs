use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{BlobError, BlobResult, BlobStore};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Filesystem backend. Keys map to paths under `root`; writes go to a temp file in the same
/// directory and are renamed into place, so readers see the old or the new object, never a mix.
pub struct FsBlobStore {
    root: PathBuf,
}

impl FsBlobStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path_for(&self, key: &str) -> BlobResult<PathBuf> {
        // Keys come from `object_key` (no `..` segment possible), but refuse anything odd
        // rather than trust the caller.
        if key.is_empty()
            || key.starts_with('/')
            || key
                .split('/')
                .any(|s| s.is_empty() || s == "." || s == "..")
        {
            return Err(BlobError::Config(format!("unsafe key {key:?}")));
        }
        Ok(self.root.join(key))
    }
}

fn io(e: std::io::Error) -> BlobError {
    BlobError::Io(e.to_string())
}

#[async_trait::async_trait]
impl BlobStore for FsBlobStore {
    async fn put(&self, key: &str, bytes: &[u8]) -> BlobResult<()> {
        let path = self.path_for(key)?;
        let dir = path.parent().expect("key has a parent");
        tokio::fs::create_dir_all(dir).await.map_err(io)?;
        let tmp = dir.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            TMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        tokio::fs::write(&tmp, bytes).await.map_err(io)?;
        if let Err(e) = tokio::fs::rename(&tmp, &path).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(io(e));
        }
        Ok(())
    }

    async fn get(&self, key: &str) -> BlobResult<Option<Vec<u8>>> {
        match tokio::fs::read(self.path_for(key)?).await {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(e)),
        }
    }

    async fn exists(&self, key: &str) -> BlobResult<bool> {
        match tokio::fs::metadata(self.path_for(key)?).await {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(io(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn put_get_replace_exists() {
        let dir = tempfile::tempdir().unwrap();
        let s = FsBlobStore::new(dir.path());
        assert!(!s.exists("a/b").await.unwrap());
        assert_eq!(s.get("a/b").await.unwrap(), None);
        s.put("a/b", b"one").await.unwrap();
        assert!(s.exists("a/b").await.unwrap());
        s.put("a/b", b"two!").await.unwrap();
        assert_eq!(s.get("a/b").await.unwrap(), Some(b"two!".to_vec()));
        // No temp files left behind.
        let n = std::fs::read_dir(dir.path().join("a")).unwrap().count();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let s = FsBlobStore::new(dir.path());
        assert!(s.put("../x", b"1").await.is_err());
        assert!(s.get("/etc/passwd").await.is_err());
        assert!(s.get("a//b").await.is_err());
    }
}
