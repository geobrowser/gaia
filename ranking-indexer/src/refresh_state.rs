//! The `trending.refresh_state` read-modify-write after Postgres leaves (plan 10.21 section 10.4).
//!
//! In `trending.refresh_agenda_listings` (0118:259-263) the full-pass decision reads
//! `last_full_pass_on`, and a full pass sets it to today, all inside one transaction under
//! `pg_advisory_xact_lock('trending.agenda_listings')`. Its only writer is that function, its
//! readers are that function, the flag-off rescore shadow (`agenda_listings_refresh.rs`) and the
//! collection 7 mirror (`state_mirror.rs`). After cutover the marker lives in geo-db collection 7
//! while the effect of a full pass (the re-scores) lands in other collections, so the marker and
//! the effect are no longer atomic. Three rules replace the transaction:
//!
//! 1. one refresh at a time: the job Lease `trending-agenda-listings` (`job_lease.rs`, `lease.rs`);
//! 2. the marker is written AFTER the work succeeds, so a crash re-runs the full pass instead of
//!    skipping it for the day;
//! 3. the marker only moves forward and the write is version-checked and fenced, so a stale or
//!    racing writer can neither lose an update nor move it back (`next_marker`).
//!
//! The Lease and fence cover (1) and (3) for a single writer; the version check is belt and braces
//! for a store that can offer it, and a refusal here costs one redundant full pass, never a wrong
//! state. Pure model; time and the Lease are inputs.

use chrono::NaiveDate;

use crate::agenda_rescore_compute::is_full_pass;
use crate::lease::{FencedStore, Grant, Refused};

/// The marker after a committed full pass: never backwards.
pub fn next_marker(previous: Option<NaiveDate>, today: NaiveDate) -> NaiveDate {
    previous.map_or(today, |p| p.max(today))
}

/// Why a marker commit was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitRefused {
    /// Someone committed since this run read: re-read and decide again.
    Stale { read: u64, now: u64 },
    /// The Lease fence said no (expired holder, or a newer holder committed).
    Fenced(Refused),
}

/// The store of the marker: a value and a version that bumps on every write.
#[derive(Debug, Default)]
pub struct MarkerStore {
    value: Option<NaiveDate>,
    version: u64,
    fence: FencedStore,
}

impl MarkerStore {
    pub fn read(&self) -> (Option<NaiveDate>, u64) {
        (self.value, self.version)
    }

    /// Record a finished full pass for `today`. Fenced by `grant` at `now_us` and refused when
    /// the store moved since `read_version`.
    pub fn commit_full_pass(
        &mut self,
        grant: Grant,
        now_us: i64,
        read_version: u64,
        today: NaiveDate,
    ) -> Result<(), CommitRefused> {
        if read_version != self.version {
            return Err(CommitRefused::Stale {
                read: read_version,
                now: self.version,
            });
        }
        self.fence
            .commit(grant, now_us, "marker")
            .map_err(CommitRefused::Fenced)?;
        self.value = Some(next_marker(self.value, today));
        self.version += 1;
        Ok(())
    }
}

/// What a run did.
#[derive(Debug, PartialEq, Eq)]
pub enum Ran {
    Incremental,
    FullPass { marker_committed: bool },
}

/// One refresh: read the marker, decide, do `work(full_pass)`, then (full pass only) commit the
/// marker. `work` failing leaves the marker alone so the next run is still a full pass.
pub fn refresh<E>(
    store: &mut MarkerStore,
    grant: Grant,
    today: NaiveDate,
    now_us: i64,
    work: impl FnOnce(bool) -> Result<(), E>,
) -> Result<Ran, E> {
    let (last, version) = store.read();
    let full = is_full_pass(last, today);
    work(full)?;
    if !full {
        return Ok(Ran::Incremental);
    }
    Ok(Ran::FullPass {
        marker_committed: store
            .commit_full_pass(grant, now_us, version, today)
            .is_ok(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lease::LeaseTable;

    const KEY: &str = "trending-agenda-listings";
    const TTL: i64 = 120_000_000;
    fn day(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    #[test]
    fn the_marker_never_moves_backwards() {
        assert_eq!(next_marker(None, day("2026-11-09")), day("2026-11-09"));
        assert_eq!(
            next_marker(Some(day("2026-11-10")), day("2026-11-09")),
            day("2026-11-10")
        );
    }

    #[test]
    fn two_overlapping_refreshes_cannot_lose_an_update_with_the_lease() {
        let mut leases = LeaseTable::default();
        let mut store = MarkerStore::default();
        let today = day("2026-11-09");
        let a = leases.acquire(KEY, "a", 0, TTL).unwrap();
        // B starts while A runs and is refused: it never reads the marker.
        assert!(leases.acquire(KEY, "b", 1, TTL).is_err());
        let ran = refresh(&mut store, a, today, 2, |full| -> Result<(), ()> {
            assert!(full);
            Ok(())
        });
        assert_eq!(
            ran,
            Ok(Ran::FullPass {
                marker_committed: true
            })
        );
        leases.release(KEY, a.token);
        // B runs afterwards, sees today's marker, and does an incremental pass.
        let b = leases.acquire(KEY, "b", 3, TTL).unwrap();
        let ran = refresh(&mut store, b, today, 4, |full| -> Result<(), ()> {
            assert!(!full);
            Ok(())
        });
        assert_eq!(ran, Ok(Ran::Incremental));
    }

    #[test]
    fn without_exclusion_the_version_check_keeps_the_first_commit() {
        // Both read (None, v0) and both decide on a full pass; only the first marker commits,
        // the second is refused as stale (one redundant full pass, no lost or reverted state).
        let mut leases = LeaseTable::default();
        let mut store = MarkerStore::default();
        let today = day("2026-11-09");
        let g1 = leases.acquire(KEY, "a", 0, TTL).unwrap();
        let (_, v_a) = store.read();
        let (_, v_b) = store.read();
        let g2 = leases.acquire(KEY, "b", TTL + 1, TTL).unwrap();
        assert!(store.commit_full_pass(g2, TTL + 2, v_b, today).is_ok());
        // A is stale on both counts: an older token and an old version.
        assert!(matches!(
            store.commit_full_pass(g1, TTL + 3, v_a, today),
            Err(CommitRefused::Stale { .. })
        ));
        assert_eq!(store.read().0, Some(today));
        assert_eq!(store.read().1, 1);
    }

    #[test]
    fn an_expired_holder_is_fenced_even_when_the_version_matches() {
        let mut leases = LeaseTable::default();
        let store = MarkerStore::default();
        let stale = leases.acquire(KEY, "a", 0, TTL).unwrap();
        let (_, v) = store.read();
        let live = leases.acquire(KEY, "b", TTL + 1, TTL).unwrap();
        // The new holder commits a different marker version path: it wrote nothing yet, so the
        // old holder's version still matches, and only the token stops it.
        let mut fenced = MarkerStore::default();
        fenced.fence.commit(live, TTL + 2, "other").unwrap();
        assert!(matches!(
            fenced.commit_full_pass(stale, 1, v, day("2026-11-09")),
            Err(CommitRefused::Fenced(Refused::Fenced { .. }))
        ));
        assert_eq!(fenced.read().0, None);
    }

    #[test]
    fn a_failed_full_pass_leaves_the_marker_so_the_next_run_repeats_it() {
        let mut leases = LeaseTable::default();
        let mut store = MarkerStore::default();
        let today = day("2026-11-09");
        let g = leases.acquire(KEY, "a", 0, TTL).unwrap();
        let ran = refresh(&mut store, g, today, 1, |_| Err("rescore failed"));
        assert_eq!(ran, Err("rescore failed"));
        assert_eq!(store.read().0, None);
        let ran = refresh(&mut store, g, today, 2, |full| -> Result<(), ()> {
            assert!(full);
            Ok(())
        });
        assert_eq!(
            ran,
            Ok(Ran::FullPass {
                marker_committed: true
            })
        );
    }

    #[test]
    fn a_new_day_is_a_full_pass_again() {
        let mut leases = LeaseTable::default();
        let mut store = MarkerStore::default();
        let g = leases.acquire(KEY, "a", 0, TTL).unwrap();
        refresh(&mut store, g, day("2026-11-09"), 1, |_| Ok::<(), ()>(())).unwrap();
        let ran = refresh(&mut store, g, day("2026-11-10"), 2, |f| {
            assert!(f);
            Ok::<(), ()>(())
        });
        assert_eq!(
            ran,
            Ok(Ran::FullPass {
                marker_committed: true
            })
        );
        assert_eq!(store.read().0, Some(day("2026-11-10")));
    }
}
