//! One-off copy of existing `ipfs_cache` rows into the blob store (zero-Postgres, plan 10.22
//! section 6). Used by the `backfill_blob` binary.
//!
//! Postgres stays the source: this only reads it. Each row is compared with the object at its
//! key through the blob-store [`Mirror`] (the same length-and-hash comparison the shadow read
//! uses) and written when the object is absent. A present object that differs is left alone
//! unless `repair` is set, because the live write-through uses "only if absent" and a differing
//! object may be newer than the row this tool read.

use blob_store::{Mirror, ShadowOutcome, ipfs_object};

/// What happened to one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowOutcome {
    /// Object was absent and is now written and read back equal.
    Written,
    /// Object was absent; dry run, nothing written.
    WouldWrite,
    /// Object already equal.
    Unchanged,
    /// Object differed and was replaced (repair) and reads back equal.
    Repaired,
    /// Object differs and was left alone (no repair), or dry run.
    Differs,
    /// A blob-store read failed, or a written object did not read back equal.
    Failed,
}

/// Running counts; printed at the end, never per row.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Counts {
    pub written: u64,
    pub would_write: u64,
    pub unchanged: u64,
    pub repaired: u64,
    pub differs: u64,
    pub failed: u64,
}

impl Counts {
    pub fn add(&mut self, o: RowOutcome) {
        match o {
            RowOutcome::Written => self.written += 1,
            RowOutcome::WouldWrite => self.would_write += 1,
            RowOutcome::Unchanged => self.unchanged += 1,
            RowOutcome::Repaired => self.repaired += 1,
            RowOutcome::Differs => self.differs += 1,
            RowOutcome::Failed => self.failed += 1,
        }
    }

    pub fn total(&self) -> u64 {
        self.written
            + self.would_write
            + self.unchanged
            + self.repaired
            + self.differs
            + self.failed
    }

    /// Process exit code: 0 clean, 1 a row could not be read or verified, 2 objects differ
    /// from Postgres and were not repaired.
    pub fn exit_code(&self) -> i32 {
        if self.failed > 0 {
            1
        } else if self.differs > 0 {
            2
        } else {
            0
        }
    }
}

/// Copy one row. `mirror` must have both flags on (see [`backfill_mirror`]).
pub async fn sync_row(
    mirror: &Mirror,
    uri: &str,
    row: &ipfs_object::Row,
    repair: bool,
    dry_run: bool,
) -> RowOutcome {
    let key = ipfs_object::key(uri);
    let bytes = ipfs_object::encode(row);
    let first = mirror.shadow_read(&key, &bytes).await;
    let replace = match first {
        Some(ShadowOutcome::Match) => return RowOutcome::Unchanged,
        Some(ShadowOutcome::Missing) => false,
        Some(ShadowOutcome::Mismatch) => {
            if !repair || dry_run {
                return RowOutcome::Differs;
            }
            true
        }
        _ => return RowOutcome::Failed,
    };
    if dry_run {
        return RowOutcome::WouldWrite;
    }
    mirror.write_through(&key, &bytes, false).await;
    // write_through swallows errors by design, so read back to know it landed.
    match mirror.shadow_read(&key, &bytes).await {
        Some(ShadowOutcome::Match) if replace => RowOutcome::Repaired,
        Some(ShadowOutcome::Match) => RowOutcome::Written,
        _ => RowOutcome::Failed,
    }
}

/// The mirror this tool uses: both flags on, regardless of the service flags, over the store
/// chosen by `BLOB_BACKEND`. `None` when no store is configured.
pub fn backfill_mirror() -> Result<Option<Mirror>, blob_store::BlobError> {
    Ok(blob_store::store_from_env()?.map(|store| {
        Mirror::new(
            "ipfs_cache",
            store,
            blob_store::MirrorFlags {
                write_through: true,
                shadow_read: true,
            },
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use blob_store::FsBlobStore;
    use std::sync::Arc;

    fn mirror(dir: &std::path::Path) -> Mirror {
        Mirror::new(
            "ipfs_cache",
            Arc::new(FsBlobStore::new(dir)),
            blob_store::MirrorFlags {
                write_through: true,
                shadow_read: true,
            },
        )
    }

    fn row(data: &[u8]) -> ipfs_object::Row {
        ipfs_object::Row {
            block: "7".into(),
            space: "0AAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE".into(),
            is_errored: false,
            name: Some("n".into()),
            data: Some(data.to_vec()),
        }
    }

    #[tokio::test]
    async fn writes_absent_then_is_idempotent() {
        let d = tempfile::tempdir().unwrap();
        let m = mirror(d.path());
        let r = row(b"abc");
        assert_eq!(
            sync_row(&m, "ipfs://Qm1", &r, false, false).await,
            RowOutcome::Written
        );
        assert_eq!(
            sync_row(&m, "ipfs://Qm1", &r, false, false).await,
            RowOutcome::Unchanged
        );
        let got = m
            .store()
            .get(&ipfs_object::key("ipfs://Qm1"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            ipfs_object::decode(&got).unwrap().data,
            Some(b"abc".to_vec())
        );
    }

    #[tokio::test]
    async fn dry_run_writes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let m = mirror(d.path());
        assert_eq!(
            sync_row(&m, "ipfs://Qm1", &row(b"abc"), false, true).await,
            RowOutcome::WouldWrite
        );
        assert!(
            !m.store()
                .exists(&ipfs_object::key("ipfs://Qm1"))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn differing_object_kept_unless_repair() {
        let d = tempfile::tempdir().unwrap();
        let m = mirror(d.path());
        sync_row(&m, "ipfs://Qm1", &row(b"old"), false, false).await;
        let new = row(b"new");
        assert_eq!(
            sync_row(&m, "ipfs://Qm1", &new, false, false).await,
            RowOutcome::Differs
        );
        assert_eq!(
            sync_row(&m, "ipfs://Qm1", &new, true, true).await,
            RowOutcome::Differs
        );
        let got = m
            .store()
            .get(&ipfs_object::key("ipfs://Qm1"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            ipfs_object::decode(&got).unwrap().data,
            Some(b"old".to_vec())
        );
        assert_eq!(
            sync_row(&m, "ipfs://Qm1", &new, true, false).await,
            RowOutcome::Repaired
        );
        assert_eq!(
            sync_row(&m, "ipfs://Qm1", &new, false, false).await,
            RowOutcome::Unchanged
        );
    }

    #[test]
    fn exit_codes() {
        let mut c = Counts::default();
        assert_eq!(c.exit_code(), 0);
        c.add(RowOutcome::Differs);
        assert_eq!(c.exit_code(), 2);
        c.add(RowOutcome::Failed);
        assert_eq!(c.exit_code(), 1);
        assert_eq!(c.total(), 2);
    }
}
