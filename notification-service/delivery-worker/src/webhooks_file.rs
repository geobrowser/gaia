//! File-backed `app_webhooks` (url + secret), mounted from a Kubernetes Secret.
//!
//! Enabled by `APP_WEBHOOKS_FILE=<path>`: a JSON array of
//! `{id, app_name, url, secret, notification_types[], space_ids[]}`. Only
//! `id`, `url` and `secret` are used here; other keys are ignored. When the
//! variable is unset the worker keeps reading `app_webhooks` from Postgres.
//!
//! The file is re-read when the cached copy is older than [`FILE_TTL`], the
//! same way the indexer refreshes its webhook cache, so a rotated Secret takes
//! effect without a restart. A failed reload keeps serving the last good copy.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::RwLock;
use uuid::Uuid;

/// How long a parsed copy of the file is reused before it is re-read.
pub const FILE_TTL: Duration = Duration::from_secs(30);

/// Env var naming the mounted JSON file. Unset or empty = Postgres path.
pub const ENV_VAR: &str = "APP_WEBHOOKS_FILE";

/// Delivery target for one webhook. `Debug` deliberately omits the secret.
#[derive(Clone, Deserialize)]
pub struct WebhookTarget {
    pub id: Uuid,
    pub url: String,
    pub secret: String,
}

impl std::fmt::Debug for WebhookTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookTarget")
            .field("id", &self.id)
            .field("url", &self.url)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// Parsed file contents, keyed by webhook id.
#[derive(Debug, Default)]
pub struct WebhookDirectory {
    by_id: HashMap<Uuid, WebhookTarget>,
}

impl WebhookDirectory {
    /// Parse the JSON array. The error text never contains file contents.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let targets: Vec<WebhookTarget> = serde_json::from_slice(bytes)
            // serde_json errors carry only line/column and the expected type,
            // never the offending value.
            .map_err(|e| format!("invalid app webhooks JSON: {e}"))?;
        Ok(Self {
            by_id: targets.into_iter().map(|t| (t.id, t)).collect(),
        })
    }

    pub fn get(&self, id: &Uuid) -> Option<&WebhookTarget> {
        self.by_id.get(id)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// TTL-cached reader for the mounted file.
pub struct WebhookFile {
    path: PathBuf,
    cache: RwLock<Option<(Instant, std::sync::Arc<WebhookDirectory>)>>,
}

impl WebhookFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            cache: RwLock::new(None),
        }
    }

    /// `Some` when `APP_WEBHOOKS_FILE` is set to a non-empty path.
    pub fn from_env() -> Option<Self> {
        let p = std::env::var(ENV_VAR).ok()?;
        let p = p.trim();
        (!p.is_empty()).then(|| Self::new(p))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The current directory, reloading when stale. On a reload failure the
    /// previous copy is returned; with no previous copy the error is returned.
    pub async fn load(&self) -> Result<std::sync::Arc<WebhookDirectory>, String> {
        if let Some((at, dir)) = self.cache.read().await.as_ref() {
            if at.elapsed() < FILE_TTL {
                return Ok(dir.clone());
            }
        }
        let parsed = match tokio::fs::read(&self.path).await {
            Ok(bytes) => WebhookDirectory::parse(&bytes),
            Err(e) => Err(format!("cannot read app webhooks file: {e}")),
        };
        let mut guard = self.cache.write().await;
        match parsed {
            Ok(dir) => {
                let dir = std::sync::Arc::new(dir);
                *guard = Some((Instant::now(), dir.clone()));
                Ok(dir)
            }
            Err(e) => match guard.as_ref() {
                Some((_, old)) => {
                    hermes_instrumentation::warn!(error = %e, "app webhooks reload failed; serving last good copy");
                    Ok(old.clone())
                }
                None => Err(e),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "00000000-0000-0000-0000-00000000000a";
    const B: &str = "00000000-0000-0000-0000-00000000000b";

    fn sample() -> String {
        format!(
            r#"[
              {{"id":"{A}","app_name":"one","url":"https://one.example/hook","secret":"s3cr3t-one",
                "notification_types":["proposal_created"],"space_ids":[]}},
              {{"id":"{B}","app_name":"two","url":"https://two.example/hook","secret":"s3cr3t-two",
                "notification_types":null,"space_ids":null}}
            ]"#
        )
    }

    #[test]
    fn parses_and_looks_up_by_id() {
        let d = WebhookDirectory::parse(sample().as_bytes()).unwrap();
        assert_eq!(d.len(), 2);
        let t = d.get(&A.parse().unwrap()).unwrap();
        assert_eq!(t.url, "https://one.example/hook");
        assert_eq!(t.secret, "s3cr3t-one");
    }

    #[test]
    fn unknown_id_is_none() {
        let d = WebhookDirectory::parse(sample().as_bytes()).unwrap();
        assert!(d.get(&Uuid::from_u128(0xdead)).is_none());
    }

    #[test]
    fn parse_error_does_not_echo_values() {
        let err = WebhookDirectory::parse(br#"[{"id":"not-a-uuid","url":"u","secret":"hunter2"}]"#)
            .unwrap_err();
        assert!(!err.contains("hunter2"), "{err}");
    }

    #[test]
    fn debug_redacts_secret() {
        let d = WebhookDirectory::parse(sample().as_bytes()).unwrap();
        let s = format!("{:?}", d);
        assert!(!s.contains("s3cr3t"), "{s}");
    }

    #[test]
    fn unset_env_means_postgres_path() {
        // Unique-to-this-test guard: the variable is never set in unit tests.
        std::env::remove_var(ENV_VAR);
        assert!(WebhookFile::from_env().is_none());
    }

    #[tokio::test]
    async fn load_reads_file_and_missing_file_errors() {
        let dir = std::env::temp_dir().join(format!("aw-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("webhooks.json");
        std::fs::write(&p, sample()).unwrap();
        let f = WebhookFile::new(&p);
        assert_eq!(f.load().await.unwrap().len(), 2);
        let missing = WebhookFile::new(dir.join("nope.json"));
        assert!(missing.load().await.is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
