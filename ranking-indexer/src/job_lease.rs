//! The Kubernetes Lease that keeps the `user_topic_interest` jobs (sweep, nightly refit,
//! not-interested replace) and the feed-signals jobs from overlapping once the Postgres advisory
//! locks go away (plan 10.21 section 10.3; the model is `lease.rs`).
//!
//! Off by default: `GEODB_JOB_LEASE` unset or `0` makes every function here a no-op, so a job
//! behaves exactly as before and the Postgres lock stays the only guard. With it on:
//!
//! * a job takes the Lease named for its lock key before doing anything. If another holder has an
//!   unexpired lease the job logs it and exits 0 (the next schedule retries). The refit and the
//!   replace may wait up to `GEODB_JOB_LEASE_WAIT_SECS` first;
//! * the lease is renewed in the background while the job runs;
//! * `leaseTransitions` is the fencing token. Immediately before each Postgres commit the job
//!   re-reads the Lease and aborts (the transaction rolls back) unless it is still the holder
//!   with the same token and an unexpired lease. A holder that stalled past expiry and was
//!   replaced therefore cannot commit;
//! * errors talking to the API with the flag on fail the job (non-zero) so a missing Role shows up
//!   as a failed Job, not as silent unguarded runs.
//!
//! Residual window: the check and the commit are two steps, so a holder that expires in the few
//! milliseconds between them can still commit. The Postgres lock in the SQL functions still
//! serializes those writes today; the lease adds exclusion across the whole job (the reads and
//! mirrors around the write) and is the owner after cutover.

use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tracing::{info, warn};

use crate::error::IndexerError;

/// `user_topic_interest` sweep, refit and not-interested replace.
pub const KEY_USER_TOPIC_INTEREST: &str = "personalization-user-topic-interest";
/// `feed_signals_sync`, including its dry-run Job.
pub const KEY_FEED_SIGNALS: &str = "personalization-feed-signals";

/// `agenda_listings_refresh` (the hourly refresh and its `trending.refresh_state` marker).
pub const KEY_AGENDA_LISTINGS: &str = "trending-agenda-listings";

const DEFAULT_TTL_SECS: i64 = 120;

/// `GEODB_JOB_LEASE` (default off).
pub fn enabled() -> bool {
    matches!(
        env::var("GEODB_JOB_LEASE").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// What the API stores for one lease, in the units the logic uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub holder: Option<String>,
    pub transitions: i64,
    pub renew_us: i64,
    pub duration_us: i64,
    pub version: String,
}

impl Record {
    fn live_at(&self, now_us: i64) -> bool {
        self.holder.is_some() && self.renew_us + self.duration_us > now_us
    }
}

#[derive(Debug)]
pub enum ApiError {
    NotFound,
    /// Create found it existing, or replace found a newer resourceVersion.
    Conflict,
    Other(String),
}

/// The three calls the logic needs from the Lease API.
pub trait LeaseApi: Send + Sync + 'static {
    fn get(&self, name: &str)
        -> impl std::future::Future<Output = Result<Record, ApiError>> + Send;
    fn create(
        &self,
        name: &str,
        r: &Record,
    ) -> impl std::future::Future<Output = Result<(), ApiError>> + Send;
    /// Replace guarded by `r.version` (the resourceVersion read).
    fn replace(
        &self,
        name: &str,
        r: &Record,
    ) -> impl std::future::Future<Output = Result<(), ApiError>> + Send;
}

/// A successful acquire. `token` is `leaseTransitions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub token: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Acquired {
    Granted(Grant),
    Busy { holder: String },
}

fn api_err(e: ApiError) -> IndexerError {
    IndexerError::Config(format!("job lease: {e:?}"))
}

/// One attempt. Same live holder renews (keeps the token); an expired or released lease is taken
/// over with the token bumped; an unexpired other holder means `Busy`.
pub async fn try_acquire<A: LeaseApi>(
    api: &A,
    name: &str,
    me: &str,
    now_us: i64,
    ttl_us: i64,
) -> Result<Acquired, ApiError> {
    let fresh = |transitions, version: String| Record {
        holder: Some(me.to_string()),
        transitions,
        renew_us: now_us,
        duration_us: ttl_us,
        version,
    };
    match api.get(name).await {
        Err(ApiError::NotFound) => match api.create(name, &fresh(1, String::new())).await {
            Ok(()) => Ok(Acquired::Granted(Grant { token: 1 })),
            Err(ApiError::Conflict) => Ok(Acquired::Busy {
                holder: "(just created)".into(),
            }),
            Err(e) => Err(e),
        },
        Err(e) => Err(e),
        Ok(r) => {
            if r.live_at(now_us) && r.holder.as_deref() != Some(me) {
                return Ok(Acquired::Busy {
                    holder: r.holder.unwrap_or_default(),
                });
            }
            let transitions = if r.live_at(now_us) {
                r.transitions
            } else {
                r.transitions + 1
            };
            match api.replace(name, &fresh(transitions, r.version)).await {
                Ok(()) => Ok(Acquired::Granted(Grant { token: transitions })),
                Err(ApiError::Conflict) => Ok(Acquired::Busy {
                    holder: "(lost the race)".into(),
                }),
                Err(e) => Err(e),
            }
        }
    }
}

/// Still the holder, with this token, and unexpired?
pub async fn still_held<A: LeaseApi>(
    api: &A,
    name: &str,
    me: &str,
    grant: &Grant,
    now_us: i64,
) -> Result<bool, ApiError> {
    let r = api.get(name).await?;
    Ok(r.holder.as_deref() == Some(me) && r.transitions == grant.token && r.live_at(now_us))
}

/// Renew in place; `Ok(false)` when it is no longer ours (do not write, the job must stop).
pub async fn renew<A: LeaseApi>(
    api: &A,
    name: &str,
    me: &str,
    grant: &Grant,
    now_us: i64,
) -> Result<bool, ApiError> {
    let mut r = api.get(name).await?;
    if r.holder.as_deref() != Some(me) || r.transitions != grant.token {
        return Ok(false);
    }
    r.renew_us = now_us;
    match api.replace(name, &r).await {
        Ok(()) => Ok(true),
        Err(ApiError::Conflict) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Give the lease up (holder cleared, token kept) if it is still ours.
pub async fn release<A: LeaseApi>(api: &A, name: &str, me: &str, grant: &Grant) {
    if let Ok(mut r) = api.get(name).await {
        if r.holder.as_deref() == Some(me) && r.transitions == grant.token {
            r.holder = None;
            let _ = api.replace(name, &r).await;
        }
    }
}

fn now_us() -> i64 {
    chrono::Utc::now().timestamp_micros()
}

/// A held lease: renews itself until released or dropped.
pub struct JobLease<A: LeaseApi> {
    api: Arc<A>,
    name: String,
    me: String,
    grant: Grant,
    lost: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl<A: LeaseApi> JobLease<A> {
    pub fn token(&self) -> i64 {
        self.grant.token
    }

    /// The fence: errors unless the lease is still ours right now.
    pub async fn check(&self) -> Result<(), IndexerError> {
        let ok = !self.lost.load(Ordering::SeqCst)
            && still_held(&*self.api, &self.name, &self.me, &self.grant, now_us())
                .await
                .map_err(api_err)?;
        if ok {
            Ok(())
        } else {
            self.lost.store(true, Ordering::SeqCst);
            Err(IndexerError::Config(format!(
                "job lease {} (token {}) is no longer held; not committing",
                self.name, self.grant.token
            )))
        }
    }

    pub async fn release(self) {
        self.stop.store(true, Ordering::SeqCst);
        release(&*self.api, &self.name, &self.me, &self.grant).await;
    }
}

/// Take the lease, waiting up to `wait` for a live holder. `Ok(None)` means busy.
pub async fn acquire<A: LeaseApi>(
    api: A,
    name: &str,
    me: &str,
    ttl_secs: i64,
    wait: Duration,
) -> Result<Option<JobLease<A>>, IndexerError> {
    let ttl_us = ttl_secs * 1_000_000;
    let deadline = std::time::Instant::now() + wait;
    let grant = loop {
        match try_acquire(&api, name, me, now_us(), ttl_us)
            .await
            .map_err(api_err)?
        {
            Acquired::Granted(g) => break g,
            Acquired::Busy { holder } => {
                if std::time::Instant::now() >= deadline {
                    info!(lease = name, %holder, "job lease is held by another run");
                    return Ok(None);
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    };
    let api = Arc::new(api);
    let lost = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    {
        let (api, lost, stop) = (api.clone(), lost.clone(), stop.clone());
        let (name, me, grant) = (name.to_string(), me.to_string(), grant.clone());
        tokio::spawn(async move {
            let every = Duration::from_secs((ttl_secs / 3).max(1) as u64);
            loop {
                tokio::time::sleep(every).await;
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                match renew(&*api, &name, &me, &grant, now_us()).await {
                    Ok(true) => {}
                    Ok(false) => {
                        warn!(lease = %name, "job lease lost; the next commit will be refused");
                        lost.store(true, Ordering::SeqCst);
                        return;
                    }
                    Err(e) => warn!(lease = %name, error = ?e, "job lease renewal failed"),
                }
            }
        });
    }
    info!(lease = name, token = grant.token, "job lease acquired");
    Ok(Some(JobLease {
        api,
        name: name.to_string(),
        me: me.to_string(),
        grant,
        lost,
        stop,
    }))
}

// ---- Kubernetes ----

pub struct KubeApi {
    api: kube::Api<k8s_openapi::api::coordination::v1::Lease>,
}

impl KubeApi {
    pub async fn from_env() -> Result<Self, IndexerError> {
        let client = kube::Client::try_default()
            .await
            .map_err(|e| IndexerError::Config(format!("job lease: kube client: {e}")))?;
        let ns = env::var("GEODB_JOB_LEASE_NAMESPACE").unwrap_or_else(|_| "gaia".into());
        Ok(Self {
            api: kube::Api::namespaced(client, &ns),
        })
    }
}

fn to_ts(us: i64) -> k8s_openapi::apimachinery::pkg::apis::meta::v1::MicroTime {
    k8s_openapi::apimachinery::pkg::apis::meta::v1::MicroTime(
        k8s_openapi::jiff::Timestamp::from_microsecond(us).unwrap_or_default(),
    )
}

fn to_lease(name: &str, r: &Record) -> k8s_openapi::api::coordination::v1::Lease {
    use k8s_openapi::api::coordination::v1::{Lease, LeaseSpec};
    Lease {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(name.to_string()),
            resource_version: (!r.version.is_empty()).then(|| r.version.clone()),
            ..Default::default()
        },
        spec: Some(LeaseSpec {
            holder_identity: r.holder.clone(),
            lease_duration_seconds: Some((r.duration_us / 1_000_000) as i32),
            lease_transitions: Some(r.transitions as i32),
            renew_time: Some(to_ts(r.renew_us)),
            acquire_time: Some(to_ts(r.renew_us)),
            ..Default::default()
        }),
    }
}

fn map_err(e: kube::Error) -> ApiError {
    match &e {
        kube::Error::Api(s) if s.code == 404 => ApiError::NotFound,
        kube::Error::Api(s) if s.code == 409 => ApiError::Conflict,
        _ => ApiError::Other(e.to_string()),
    }
}

impl LeaseApi for KubeApi {
    async fn get(&self, name: &str) -> Result<Record, ApiError> {
        let l = self.api.get(name).await.map_err(map_err)?;
        let spec = l.spec.unwrap_or_default();
        Ok(Record {
            holder: spec.holder_identity.filter(|h| !h.is_empty()),
            transitions: spec.lease_transitions.unwrap_or(0) as i64,
            renew_us: spec.renew_time.map(|t| t.0.as_microsecond()).unwrap_or(0),
            duration_us: spec.lease_duration_seconds.unwrap_or(0) as i64 * 1_000_000,
            version: l.metadata.resource_version.unwrap_or_default(),
        })
    }
    async fn create(&self, name: &str, r: &Record) -> Result<(), ApiError> {
        self.api
            .create(&Default::default(), &to_lease(name, r))
            .await
            .map(|_| ())
            .map_err(map_err)
    }
    async fn replace(&self, name: &str, r: &Record) -> Result<(), ApiError> {
        self.api
            .replace(name, &Default::default(), &to_lease(name, r))
            .await
            .map(|_| ())
            .map_err(map_err)
    }
}

// ---- Process-wide fence, called right before each Postgres commit ----

static HELD: OnceLock<JobLease<KubeApi>> = OnceLock::new();

/// The outcome of `guard`.
pub enum Guard {
    /// Flag off: carry on exactly as before.
    Off,
    /// Lease held; call `finish` when done.
    Held,
    /// Another run holds it: log and exit 0.
    Busy,
}

/// Take the lease for `key` if `GEODB_JOB_LEASE` is on and install it as the process fence.
/// `wait_default_secs` is how long this job waits for a live holder (`GEODB_JOB_LEASE_WAIT_SECS`
/// overrides; the sweep passes 0 and skips).
pub async fn guard(key: &str, wait_default_secs: u64) -> Result<Guard, IndexerError> {
    if !enabled() {
        return Ok(Guard::Off);
    }
    let me = env::var("HOSTNAME").unwrap_or_else(|_| format!("pid-{}", std::process::id()));
    let ttl = env::var("GEODB_JOB_LEASE_TTL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_TTL_SECS);
    let wait = env::var("GEODB_JOB_LEASE_WAIT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(wait_default_secs);
    let api = KubeApi::from_env().await?;
    match acquire(api, key, &me, ttl, Duration::from_secs(wait)).await? {
        None => Ok(Guard::Busy),
        Some(l) => {
            let _ = HELD.set(l);
            Ok(Guard::Held)
        }
    }
}

/// The fencing token of the held lease, `None` when no lease is held (flag off, or in tests).
pub fn held_token() -> Option<u64> {
    HELD.get().and_then(|l| u64::try_from(l.token()).ok())
}

/// The fence. A no-op when no lease is held (flag off, or in tests).
pub async fn fence() -> Result<(), IndexerError> {
    match HELD.get() {
        Some(l) => l.check().await,
        None => Ok(()),
    }
}

/// Mark the lease free so the next run need not wait out the expiry. Best effort.
pub async fn finish() {
    if let Some(l) = HELD.get() {
        l.stop.store(true, Ordering::SeqCst);
        release(&*l.api, &l.name, &l.me, &l.grant).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory Lease API with resourceVersion conflicts, like the real one.
    #[derive(Default, Clone)]
    struct Fake {
        m: Arc<Mutex<HashMap<String, (Record, u64)>>>,
    }

    impl LeaseApi for Fake {
        async fn get(&self, name: &str) -> Result<Record, ApiError> {
            let m = self.m.lock().unwrap();
            let (r, v) = m.get(name).ok_or(ApiError::NotFound)?;
            Ok(Record {
                version: v.to_string(),
                ..r.clone()
            })
        }
        async fn create(&self, name: &str, r: &Record) -> Result<(), ApiError> {
            let mut m = self.m.lock().unwrap();
            if m.contains_key(name) {
                return Err(ApiError::Conflict);
            }
            m.insert(name.into(), (r.clone(), 1));
            Ok(())
        }
        async fn replace(&self, name: &str, r: &Record) -> Result<(), ApiError> {
            let mut m = self.m.lock().unwrap();
            let (_, v) = m.get(name).ok_or(ApiError::NotFound)?;
            if r.version != v.to_string() {
                return Err(ApiError::Conflict);
            }
            let v = *v + 1;
            m.insert(name.into(), (r.clone(), v));
            Ok(())
        }
    }

    const K: &str = KEY_USER_TOPIC_INTEREST;
    const TTL: i64 = 120_000_000;

    #[tokio::test]
    async fn two_overlapping_runs_serialize() {
        let api = Fake::default();
        let a = try_acquire(&api, K, "sweep", 0, TTL).await.unwrap();
        let Acquired::Granted(ga) = a else { panic!() };
        // The refit arrives while the sweep holds it.
        assert_eq!(
            try_acquire(&api, K, "refit", 1, TTL).await.unwrap(),
            Acquired::Busy {
                holder: "sweep".into()
            }
        );
        release(&api, K, "sweep", &ga).await;
        // Now it can go, strictly after, with a higher token.
        let Acquired::Granted(gb) = try_acquire(&api, K, "refit", 2, TTL).await.unwrap() else {
            panic!()
        };
        assert!(gb.token > ga.token);
    }

    #[tokio::test]
    async fn expired_holder_is_fenced_and_dead_holder_recovered() {
        let api = Fake::default();
        let Acquired::Granted(old) = try_acquire(&api, K, "refit", 0, TTL).await.unwrap() else {
            panic!()
        };
        assert!(still_held(&api, K, "refit", &old, TTL - 1).await.unwrap());
        // Past expiry the holder's own check fails...
        assert!(!still_held(&api, K, "refit", &old, TTL + 1).await.unwrap());
        // ...a successor takes over with a higher token...
        let Acquired::Granted(new) = try_acquire(&api, K, "sweep", TTL + 1, TTL).await.unwrap()
        else {
            panic!()
        };
        assert_eq!(new.token, old.token + 1);
        // ...and even if the old holder checks with a clock that says it is still in time, it is
        // not the holder any more, so it cannot commit; nor can it renew.
        assert!(!still_held(&api, K, "refit", &old, 1).await.unwrap());
        assert!(!renew(&api, K, "refit", &old, 1).await.unwrap());
        assert!(still_held(&api, K, "sweep", &new, TTL + 2).await.unwrap());
    }

    #[tokio::test]
    async fn racing_takeovers_have_one_winner() {
        let api = Fake::default();
        try_acquire(&api, K, "a", 0, TTL).await.unwrap();
        // Both read the expired record, then both replace: the second gets a conflict.
        let r = api.get(K).await.unwrap();
        let mut a = r.clone();
        a.holder = Some("b".into());
        a.transitions += 1;
        let mut c = r.clone();
        c.holder = Some("c".into());
        c.transitions += 1;
        assert!(api.replace(K, &a).await.is_ok());
        assert!(matches!(api.replace(K, &c).await, Err(ApiError::Conflict)));
    }

    #[tokio::test]
    async fn renew_keeps_the_token_and_release_lets_the_next_in() {
        let api = Fake::default();
        let Acquired::Granted(g) = try_acquire(&api, K, "a", 0, TTL).await.unwrap() else {
            panic!()
        };
        assert!(renew(&api, K, "a", &g, TTL / 2).await.unwrap());
        assert!(still_held(&api, K, "a", &g, TTL + 5).await.unwrap());
        release(&api, K, "a", &g).await;
        assert!(matches!(
            try_acquire(&api, K, "b", TTL / 2 + 1, TTL).await.unwrap(),
            Acquired::Granted(_)
        ));
    }

    #[tokio::test]
    async fn the_flag_defaults_off_and_fence_is_a_no_op() {
        assert!(matches!(guard(K, 0).await, Ok(Guard::Off)) || enabled());
        assert!(fence().await.is_ok() || enabled());
    }
}
