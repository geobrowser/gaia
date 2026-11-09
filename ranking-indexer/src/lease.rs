//! Pure model of the lease that replaces `pg_try_advisory_xact_lock` for the jobs
//! that share `personalization.user_topic_interest` (the sweep, the nightly refit
//! and the not-interested replace; plan 10.21 section 10, row 1).
//!
//! The real owner is a Kubernetes `Lease` (one per lock key). This module models
//! exactly the three properties the jobs rely on, with time as an input so the
//! tests are deterministic:
//!
//! * mutual exclusion: `acquire` fails while an unexpired holder exists;
//! * dead-holder recovery: once `expires_at_us` passes, the next `acquire` wins;
//! * fencing: every successful acquire (a takeover or a fresh grant) bumps a
//!   monotone token (the Lease's `leaseTransitions`). The guarded store records the
//!   highest token it has committed and refuses a lower one, so a holder that
//!   paused past its expiry cannot write after its successor.
//!
//! Decision (plan 10.21 section 10.3): a Kubernetes Lease per lock key, with
//! `leaseTransitions` as the fencing token, enforced at the store commit. A keyed
//! geo-db lock record was rejected because geo-db has no compare-and-set (records
//! are last-write-wins puts) and the jobs would then depend on geo-db being up to
//! take a lock; a single job was rejected because the cadences differ (2-minute
//! sweep, nightly refit, event-driven replace). The same table serves
//! `feed_signals_compute::Lease` (ADVISORY-2) under the key
//! `personalization.feed_signals`. Not wired into any job yet: that needs a
//! `coordination.k8s.io` Role for the job service accounts.
//!
//! Exclusion alone is not enough: a holder can stall (GC, a slow pod) past its
//! expiry, wake up, and still believe it holds the lock. Only the store can say no.

use std::collections::HashMap;

/// A grant. Tokens are strictly increasing per key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grant {
    pub token: u64,
    pub expires_at_us: i64,
}

/// Why an acquire or a commit was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Somebody else holds an unexpired lease; try again later.
    Held { holder: String },
    /// The caller's token is lower than one the store already committed.
    Fenced { token: u64, committed: u64 },
    /// The caller's own lease ran out before the commit (it must not write).
    Expired,
}

#[derive(Debug, Clone)]
struct Slot {
    holder: String,
    token: u64,
    expires_at_us: i64,
}

/// The lease table: key to current slot. Mirrors one Lease object per key.
#[derive(Debug, Default)]
pub struct LeaseTable {
    slots: HashMap<String, Slot>,
    last_token: HashMap<String, u64>,
}

impl LeaseTable {
    /// Take the lease for `key`, or fail if an unexpired other holder has it.
    /// Re-acquiring by the same live holder is a renewal: same token, new expiry.
    pub fn acquire(
        &mut self,
        key: &str,
        who: &str,
        now_us: i64,
        ttl_us: i64,
    ) -> Result<Grant, Refused> {
        if let Some(s) = self.slots.get_mut(key) {
            if s.expires_at_us > now_us {
                if s.holder == who {
                    s.expires_at_us = now_us + ttl_us;
                    return Ok(Grant {
                        token: s.token,
                        expires_at_us: s.expires_at_us,
                    });
                }
                return Err(Refused::Held {
                    holder: s.holder.clone(),
                });
            }
        }
        let token = self.last_token.get(key).copied().unwrap_or(0) + 1;
        self.last_token.insert(key.to_string(), token);
        let expires_at_us = now_us + ttl_us;
        self.slots.insert(
            key.to_string(),
            Slot {
                holder: who.to_string(),
                token,
                expires_at_us,
            },
        );
        Ok(Grant {
            token,
            expires_at_us,
        })
    }

    /// Give the lease up early. Only the current token may release.
    pub fn release(&mut self, key: &str, token: u64) {
        if self.slots.get(key).is_some_and(|s| s.token == token) {
            self.slots.remove(key);
        }
    }
}

/// A store whose writes are fenced by token (the cursor row, or a replaced table).
#[derive(Debug, Default)]
pub struct FencedStore {
    committed_token: u64,
    pub writes: Vec<(u64, String)>,
}

impl FencedStore {
    /// Commit `what` under `grant`. Refused when the holder's own lease has
    /// lapsed at `now_us`, or when a newer holder already committed.
    pub fn commit(&mut self, grant: Grant, now_us: i64, what: &str) -> Result<(), Refused> {
        if grant.expires_at_us <= now_us {
            return Err(Refused::Expired);
        }
        if grant.token < self.committed_token {
            return Err(Refused::Fenced {
                token: grant.token,
                committed: self.committed_token,
            });
        }
        self.committed_token = grant.token;
        self.writes.push((grant.token, what.to_string()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "personalization.user_topic_interest";
    const TTL: i64 = 10_000_000;

    #[test]
    fn overlapping_runs_serialize() {
        let mut t = LeaseTable::default();
        let mut s = FencedStore::default();
        let sweep = t.acquire(KEY, "sweep", 0, TTL).unwrap();
        // The refit and the replace arrive while the sweep runs: both refused.
        assert_eq!(
            t.acquire(KEY, "refit", 1, TTL),
            Err(Refused::Held {
                holder: "sweep".into()
            })
        );
        assert!(matches!(
            t.acquire(KEY, "replace", 2, TTL),
            Err(Refused::Held { .. })
        ));
        s.commit(sweep, 3, "sweep").unwrap();
        t.release(KEY, sweep.token);
        // Once released, the next one runs, strictly after.
        let refit = t.acquire(KEY, "refit", 4, TTL).unwrap();
        assert!(refit.token > sweep.token);
        s.commit(refit, 5, "refit").unwrap();
        assert_eq!(
            s.writes.iter().map(|w| w.1.as_str()).collect::<Vec<_>>(),
            ["sweep", "refit"]
        );
    }

    #[test]
    fn dead_holder_is_recovered_after_expiry() {
        let mut t = LeaseTable::default();
        let dead = t.acquire(KEY, "refit", 0, TTL).unwrap();
        assert!(t.acquire(KEY, "sweep", TTL - 1, TTL).is_err());
        let next = t.acquire(KEY, "sweep", TTL, TTL).unwrap();
        assert_eq!(next.token, dead.token + 1);
    }

    #[test]
    fn expired_holder_is_fenced() {
        let mut t = LeaseTable::default();
        let mut s = FencedStore::default();
        let stale = t.acquire(KEY, "refit", 0, TTL).unwrap();
        // The refit stalls past its expiry; the sweep takes over and commits.
        let live = t.acquire(KEY, "sweep", TTL + 1, TTL).unwrap();
        s.commit(live, TTL + 2, "sweep").unwrap();
        // The stalled refit wakes up. Its own clock check refuses it...
        assert_eq!(s.commit(stale, TTL + 3, "refit"), Err(Refused::Expired));
        // ...and even if it lied about the time, the store refuses the old token.
        let lied = Grant {
            expires_at_us: i64::MAX,
            ..stale
        };
        assert_eq!(
            s.commit(lied, TTL + 3, "refit"),
            Err(Refused::Fenced {
                token: stale.token,
                committed: live.token
            })
        );
        assert_eq!(s.writes.len(), 1);
    }

    #[test]
    fn renewal_keeps_the_token_and_stale_release_is_ignored() {
        let mut t = LeaseTable::default();
        let a = t.acquire(KEY, "refit", 0, TTL).unwrap();
        let again = t.acquire(KEY, "refit", TTL / 2, TTL).unwrap();
        assert_eq!(a.token, again.token);
        assert!(again.expires_at_us > a.expires_at_us);
        t.release(KEY, a.token + 7);
        assert!(t.acquire(KEY, "sweep", TTL / 2 + 1, TTL).is_err());
    }

    #[test]
    fn keys_are_independent() {
        let mut t = LeaseTable::default();
        t.acquire(KEY, "sweep", 0, TTL).unwrap();
        assert!(t
            .acquire("personalization.feed_signals", "cron", 0, TTL)
            .is_ok());
    }
}
