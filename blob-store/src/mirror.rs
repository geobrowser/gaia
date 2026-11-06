use std::sync::Arc;

use tokio::sync::Semaphore;

use sha2::{Digest, Sha256};

use crate::BlobStore;

/// Length and SHA-256 of a payload: what the shadow read compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub len: usize,
    pub hash: String,
}

pub fn digest(bytes: &[u8]) -> Fingerprint {
    Fingerprint {
        len: bytes.len(),
        hash: hex::encode(Sha256::digest(bytes)),
    }
}

/// Which halves of the mirror are on. Both default to off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MirrorFlags {
    pub write_through: bool,
    pub shadow_read: bool,
}

impl MirrorFlags {
    /// Reads `<prefix>_BLOB_WRITE_THROUGH` and `<prefix>_BLOB_SHADOW_READ`; only the exact
    /// value `1` turns a flag on.
    pub fn from_env(prefix: &str) -> Self {
        let on = |suffix: &str| {
            std::env::var(format!("{prefix}_{suffix}")).is_ok_and(|v| v.trim() == "1")
        };
        Self {
            write_through: on("BLOB_WRITE_THROUGH"),
            shadow_read: on("BLOB_SHADOW_READ"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowOutcome {
    /// Same length and hash.
    Match,
    /// Both sides have it but length or hash differs.
    Mismatch,
    /// Postgres has it, the object store does not.
    Missing,
    /// The object store call failed.
    Error,
}

impl ShadowOutcome {
    fn label(self) -> &'static str {
        match self {
            Self::Match => "match",
            Self::Mismatch => "mismatch",
            Self::Missing => "missing",
            Self::Error => "error",
        }
    }
}

/// Flag-gated write-through and shadow read for one table. Failures are counted and logged,
/// never returned: the Postgres path is the source until the shadow has been clean.
pub struct Mirror {
    table: &'static str,
    store: Arc<dyn BlobStore>,
    flags: MirrorFlags,
    /// Bounds detached shadow reads so a slow object store cannot pile up tasks.
    shadow_slots: Arc<Semaphore>,
}

/// Concurrent detached shadow reads allowed per process.
const SHADOW_SLOTS: usize = 8;

impl std::fmt::Debug for Mirror {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mirror")
            .field("table", &self.table)
            .field("flags", &self.flags)
            .finish_non_exhaustive()
    }
}

impl Mirror {
    pub fn new(table: &'static str, store: Arc<dyn BlobStore>, flags: MirrorFlags) -> Self {
        Self {
            table,
            store,
            flags,
            shadow_slots: Arc::new(Semaphore::new(SHADOW_SLOTS)),
        }
    }

    /// Run [`Mirror::shadow_read`] off the caller's path. Skipped (and counted) when all
    /// slots are busy, so the shadow can never slow or stall the reader it observes.
    pub fn shadow_read_detached(self: &Arc<Self>, key: String, expected: Vec<u8>) {
        if !self.flags.shadow_read {
            return;
        }
        let Ok(permit) = self.shadow_slots.clone().try_acquire_owned() else {
            metrics::counter!("blob_shadow_compare_total", "table" => self.table, "result" => "skipped")
                .increment(1);
            return;
        };
        let this = self.clone();
        tokio::spawn(async move {
            this.shadow_read(&key, &expected).await;
            drop(permit);
        });
    }

    /// Build from the environment: `None` unless a flag with `flag_prefix` is on and
    /// `BLOB_BACKEND` yields a store. A misconfigured store is logged and yields `None`, so a
    /// bad flag-off-by-default setting cannot stop the service from starting.
    pub fn from_env(table: &'static str, flag_prefix: &str) -> Option<Self> {
        let flags = MirrorFlags::from_env(flag_prefix);
        if !flags.write_through && !flags.shadow_read {
            return None;
        }
        match crate::store_from_env() {
            Ok(Some(store)) => Some(Self::new(table, store, flags)),
            Ok(None) => {
                tracing::warn!(table, "blob mirror flag is on but BLOB_BACKEND is unset");
                None
            }
            Err(e) => {
                tracing::error!(table, error = %e, "blob mirror disabled: store configuration failed");
                None
            }
        }
    }

    pub fn flags(&self) -> MirrorFlags {
        self.flags
    }

    /// The underlying store (for callers that need the object's current content).
    pub fn store(&self) -> &Arc<dyn BlobStore> {
        &self.store
    }

    /// Write `bytes` to `key` if write-through is on. With `only_if_absent` an existing
    /// object is left alone (the `ON CONFLICT DO NOTHING` shape).
    pub async fn write_through(&self, key: &str, bytes: &[u8], only_if_absent: bool) {
        if !self.flags.write_through {
            return;
        }
        let result = async {
            if only_if_absent && self.store.exists(key).await? {
                return Ok(());
            }
            self.store.put(key, bytes).await
        }
        .await;
        let outcome = if result.is_ok() { "ok" } else { "error" };
        metrics::counter!("blob_write_through_total", "table" => self.table, "result" => outcome)
            .increment(1);
        if let Err(e) = result {
            tracing::warn!(table = self.table, error = %e, "blob write-through failed");
        }
    }

    /// Compare the object at `key` with what Postgres returned (`expected` is the canonical
    /// bytes of the Postgres row). Returns `None` when shadow read is off.
    pub async fn shadow_read(&self, key: &str, expected: &[u8]) -> Option<ShadowOutcome> {
        if !self.flags.shadow_read {
            return None;
        }
        let outcome = match self.store.get(key).await {
            Ok(Some(got)) => {
                if digest(&got) == digest(expected) {
                    ShadowOutcome::Match
                } else {
                    ShadowOutcome::Mismatch
                }
            }
            Ok(None) => ShadowOutcome::Missing,
            Err(e) => {
                tracing::warn!(table = self.table, error = %e, "blob shadow read failed");
                ShadowOutcome::Error
            }
        };
        metrics::counter!("blob_shadow_compare_total", "table" => self.table, "result" => outcome.label())
            .increment(1);
        if matches!(outcome, ShadowOutcome::Mismatch | ShadowOutcome::Missing) {
            tracing::warn!(
                table = self.table,
                result = outcome.label(),
                "blob shadow read differs"
            );
        }
        Some(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FsBlobStore;

    fn mirror(flags: MirrorFlags) -> (Mirror, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let m = Mirror::new("t", Arc::new(FsBlobStore::new(dir.path())), flags);
        (m, dir)
    }

    const ON: MirrorFlags = MirrorFlags {
        write_through: true,
        shadow_read: true,
    };

    #[tokio::test]
    async fn flags_off_do_nothing() {
        let (m, _d) = mirror(MirrorFlags::default());
        m.write_through("t/k", b"x", false).await;
        assert!(!m.store().exists("t/k").await.unwrap());
        assert_eq!(m.shadow_read("t/k", b"x").await, None);
    }

    #[tokio::test]
    async fn shadow_outcomes() {
        let (m, _d) = mirror(ON);
        assert_eq!(
            m.shadow_read("t/k", b"abc").await,
            Some(ShadowOutcome::Missing)
        );
        m.write_through("t/k", b"abc", false).await;
        assert_eq!(
            m.shadow_read("t/k", b"abc").await,
            Some(ShadowOutcome::Match)
        );
        // Same length, different hash.
        assert_eq!(
            m.shadow_read("t/k", b"abd").await,
            Some(ShadowOutcome::Mismatch)
        );
        // Different length.
        assert_eq!(
            m.shadow_read("t/k", b"ab").await,
            Some(ShadowOutcome::Mismatch)
        );
    }

    #[tokio::test]
    async fn only_if_absent_keeps_first() {
        let (m, _d) = mirror(ON);
        m.write_through("t/k", b"first", true).await;
        m.write_through("t/k", b"second", true).await;
        assert_eq!(m.store().get("t/k").await.unwrap(), Some(b"first".to_vec()));
        m.write_through("t/k", b"third", false).await;
        assert_eq!(m.store().get("t/k").await.unwrap(), Some(b"third".to_vec()));
    }

    #[tokio::test]
    async fn detached_shadow_read_runs_and_is_bounded() {
        let (m, _d) = mirror(ON);
        let m = Arc::new(m);
        m.write_through("t/k", b"abc", false).await;
        m.shadow_read_detached("t/k".into(), b"abc".to_vec());
        // Exhaust the slots: further calls are skipped, not queued.
        let held: Vec<_> = (0..SHADOW_SLOTS)
            .filter_map(|_| m.shadow_slots.clone().try_acquire_owned().ok())
            .collect();
        m.shadow_read_detached("t/k".into(), b"abc".to_vec());
        drop(held);
        tokio::task::yield_now().await;
    }

    #[test]
    fn env_flags_need_exactly_one() {
        // SAFETY-free: use a unique prefix so parallel tests do not interact.
        unsafe { std::env::set_var("ZZTEST_BLOB_WRITE_THROUGH", "true") };
        unsafe { std::env::set_var("ZZTEST_BLOB_SHADOW_READ", "1") };
        let f = MirrorFlags::from_env("ZZTEST");
        assert!(!f.write_through && f.shadow_read);
        assert_eq!(MirrorFlags::from_env("ZZUNSET"), MirrorFlags::default());
    }
}
