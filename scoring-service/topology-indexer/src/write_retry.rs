//! Retry, halt and skip policy for indexer writes (GEO-2884, GEO-3101).
//!
//! **An indexer never moves past a message whose write failed for a reason that
//! could go away.** A failed write is retried with bounded exponential backoff. If
//! it still fails, the caller does not commit the offset and the process exits
//! non-zero, so Kubernetes restarts the pod on the last committed offset and the
//! message is read again. A persistent failure becomes a visible crash-loop rather
//! than a silently skipped batch.
//!
//! The exception is a **poison message**: one whose write can never succeed,
//! however often it is retried. Halting on it would block the partition forever, so
//! it is skipped, committed past, counted and alerted on. That is only done for
//! errors that are provably about the message's own data (see [`classify_sqlx`]).
//! Anything else, including errors nobody has classified, is treated as transient:
//! a wrong "transient" costs a crash-loop someone has to look at, a wrong
//! "permanent" costs data that nobody knows is missing.
//!
//! A permanent error that repeats on message after message is not a poison message,
//! it is a systemic fault (a migration that tightened a column, say) that would
//! otherwise skip everything. [`SkipGuard`] halts once too many permanent failures
//! arrive in a row.
//!
//! The same file is copied into kg-indexer, vote-indexer, topology-indexer and
//! notification-indexer. Each image builds its crate standalone (see its
//! Dockerfile), so a shared crate would mean new COPY lines and CI path filters in
//! four places; keep the copies identical instead. Not every crate uses every
//! item, hence the `dead_code` allowance.

#![allow(dead_code)]

use std::future::Future;
use std::ops::Range;
use std::time::Duration;

/// Whether a failed write is worth trying again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// Could succeed on a later attempt: retry, then halt without committing.
    Transient,
    /// Can never succeed for this message: skip it, commit past, count and alert.
    Permanent,
}

/// Classify a database error.
///
/// Permanent only for SQLSTATE classes that describe the data being written:
///
/// * `22` data exception: invalid text representation, value out of range,
///   string too long, invalid JSON and so on;
/// * `23` integrity constraint violation: unique, foreign key, not null, check.
///
/// Both fail identically every time the same row is offered. Everything else is
/// transient, including the classes a retry clearly fixes (`08` connection, `40`
/// serialization failure and deadlock, `53` insufficient resources, `55P03` lock
/// not available, `57` operator intervention and `57014` statement timeout) and the
/// ones a retry does not fix but that are not the message's fault either (`42`
/// undefined table or column after a bad migration, `XX` internal error). Skipping
/// on those would drop every message until someone noticed; halting stops the loss
/// and pages.
///
/// Non-database sqlx errors (pool timeout, I/O, TLS, protocol, a column that no
/// longer decodes into the Rust type) are transient for the same reason.
pub fn classify_sqlx(error: &sqlx::Error) -> ErrorClass {
    match error {
        sqlx::Error::Database(db) => db
            .code()
            .as_deref()
            .map(classify_sqlstate)
            .unwrap_or(ErrorClass::Transient),
        _ => ErrorClass::Transient,
    }
}

/// Classify a five-character Postgres SQLSTATE. See [`classify_sqlx`].
pub fn classify_sqlstate(code: &str) -> ErrorClass {
    if code.starts_with("22") || code.starts_with("23") {
        ErrorClass::Permanent
    } else {
        ErrorClass::Transient
    }
}

/// What to do after a failed attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Sleep for this long, then try again.
    Retry(Duration),
    /// Transient and out of attempts: leave the offset uncommitted and exit.
    Halt,
    /// Permanent: skip the message and commit past it.
    Skip,
}

/// Bounded exponential backoff for one write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total attempts, including the first. Always at least 1.
    pub max_attempts: u32,
    /// Delay after the first failed attempt; doubles on each further one.
    pub initial_backoff: Duration,
    /// Ceiling for a single delay.
    pub max_backoff: Duration,
}

impl Default for RetryPolicy {
    /// Five attempts, waiting 2s, 4s, 8s and 16s between them: about 30 seconds
    /// before halting, which rides out a pooler restart or a failover without
    /// holding a message for minutes.
    fn default() -> Self {
        Self {
            max_attempts: 5,
            initial_backoff: Duration::from_secs(2),
            max_backoff: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    /// Read `INDEXER_WRITE_MAX_ATTEMPTS`, `INDEXER_WRITE_BACKOFF_MS` and
    /// `INDEXER_WRITE_BACKOFF_MAX_MS`, falling back to the defaults for anything
    /// unset, unparseable or zero.
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// [`RetryPolicy::from_env`] with the environment injected, for tests.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let default = Self::default();
        let positive = |key: &str| {
            lookup(key)
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|v| *v > 0)
        };
        let max_attempts = positive("INDEXER_WRITE_MAX_ATTEMPTS")
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(default.max_attempts);
        let initial_backoff = positive("INDEXER_WRITE_BACKOFF_MS")
            .map(Duration::from_millis)
            .unwrap_or(default.initial_backoff);
        let max_backoff = positive("INDEXER_WRITE_BACKOFF_MAX_MS")
            .map(Duration::from_millis)
            .unwrap_or(default.max_backoff)
            .max(initial_backoff);
        Self {
            max_attempts,
            initial_backoff,
            max_backoff,
        }
    }

    /// The delay after failed attempt `attempt` (1-based).
    pub fn backoff(&self, attempt: u32) -> Duration {
        let doublings = attempt.saturating_sub(1).min(20);
        self.initial_backoff
            .saturating_mul(1u32 << doublings)
            .min(self.max_backoff)
    }

    /// Decide what follows failed attempt `attempt` (1-based).
    pub fn decide(&self, attempt: u32, class: ErrorClass) -> Decision {
        match class {
            ErrorClass::Permanent => Decision::Skip,
            ErrorClass::Transient if attempt < self.max_attempts.max(1) => {
                Decision::Retry(self.backoff(attempt))
            }
            ErrorClass::Transient => Decision::Halt,
        }
    }
}

/// How a retried write ended.
#[derive(Debug)]
pub enum Outcome<T, E> {
    /// The write succeeded, possibly after retries.
    Written(T),
    /// A permanent error: the caller skips the message.
    Skip(E),
    /// A transient error outlasted every attempt: the caller must not commit, and
    /// should call [`halt`].
    Halt { error: E, attempts: u32 },
}

/// Run `op` under `policy`, retrying transient failures.
///
/// `on_retry(attempt, &error, delay)` runs before each sleep, which is where the
/// caller logs and bumps its `*_write_retries_total` counter.
pub async fn run<T, E, F, Fut>(
    policy: &RetryPolicy,
    classify: impl Fn(&E) -> ErrorClass,
    mut on_retry: impl FnMut(u32, &E, Duration),
    mut op: F,
) -> Outcome<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut attempt: u32 = 1;
    loop {
        match op().await {
            Ok(value) => return Outcome::Written(value),
            Err(error) => match policy.decide(attempt, classify(&error)) {
                Decision::Skip => return Outcome::Skip(error),
                Decision::Halt => {
                    return Outcome::Halt {
                        error,
                        attempts: attempt,
                    };
                }
                Decision::Retry(delay) => {
                    on_retry(attempt, &error, delay);
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
            },
        }
    }
}

/// What happened to each item of a batch written with [`write_batch`].
#[derive(Debug)]
pub struct BatchReport<E> {
    /// Items written.
    pub written: usize,
    /// Items skipped for a permanent error, by index.
    pub skipped: Vec<(usize, E)>,
}

/// How [`write_batch`] ended.
#[derive(Debug)]
pub enum BatchOutcome<E> {
    /// Every item was written or skipped: the caller commits the whole batch.
    Done(BatchReport<E>),
    /// Item `index` could not be written and the caller must halt. Items before
    /// it are finished (written or skipped) and may be committed; `index` and
    /// everything after it must not be.
    Halt {
        index: usize,
        error: E,
        attempts: u32,
        report: BatchReport<E>,
    },
}

/// Write `len` items, isolating a poison item instead of losing its neighbours.
///
/// `write` is handed the index range to write; the caller slices its own data, so
/// the future can borrow it. With `atomic_first`, the whole range is offered once
/// (retried as usual), which is the fast path for a batch that is one
/// transaction. If that fails permanently, or without `atomic_first`, each item is
/// written on its own, so only the item that can never be written is skipped.
/// `guard` turns a run of permanent failures into a halt.
pub async fn write_batch<E, W, Fut>(
    len: usize,
    policy: &RetryPolicy,
    classify: impl Fn(&E) -> ErrorClass,
    guard: &mut SkipGuard,
    mut on_retry: impl FnMut(u32, &E, Duration),
    atomic_first: bool,
    mut write: W,
) -> BatchOutcome<E>
where
    W: FnMut(Range<usize>) -> Fut,
    Fut: Future<Output = Result<(), E>>,
{
    let mut report = BatchReport {
        written: 0,
        skipped: Vec::new(),
    };
    if len == 0 {
        return BatchOutcome::Done(report);
    }

    if atomic_first {
        match run(policy, &classify, &mut on_retry, || write(0..len)).await {
            Outcome::Written(()) => {
                guard.record_success();
                report.written = len;
                return BatchOutcome::Done(report);
            }
            Outcome::Halt { error, attempts } => {
                return BatchOutcome::Halt {
                    index: 0,
                    error,
                    attempts,
                    report,
                };
            }
            // Something in the batch can never be written; find it.
            Outcome::Skip(_) => {}
        }
    }

    for index in 0..len {
        match run(policy, &classify, &mut on_retry, || write(index..index + 1)).await {
            Outcome::Written(()) => {
                guard.record_success();
                report.written += 1;
            }
            Outcome::Skip(error) => {
                if guard.record_skip() {
                    return BatchOutcome::Halt {
                        index,
                        error,
                        attempts: 1,
                        report,
                    };
                }
                report.skipped.push((index, error));
            }
            Outcome::Halt { error, attempts } => {
                return BatchOutcome::Halt {
                    index,
                    error,
                    attempts,
                    report,
                };
            }
        }
    }
    BatchOutcome::Done(report)
}

/// Counts permanent skips in a row, and says when they stop looking like poison
/// messages and start looking like a systemic fault.
#[derive(Debug, Clone)]
pub struct SkipGuard {
    consecutive: u32,
    limit: u32,
}

impl SkipGuard {
    /// Default ceiling on consecutive permanent skips before halting.
    pub const DEFAULT_LIMIT: u32 = 10;

    pub fn new(limit: u32) -> Self {
        Self {
            consecutive: 0,
            limit: limit.max(1),
        }
    }

    /// Read `INDEXER_MAX_CONSECUTIVE_SKIPS`, defaulting to [`SkipGuard::DEFAULT_LIMIT`].
    pub fn from_env() -> Self {
        Self::new(
            std::env::var("INDEXER_MAX_CONSECUTIVE_SKIPS")
                .ok()
                .and_then(|v| v.trim().parse::<u32>().ok())
                .filter(|v| *v > 0)
                .unwrap_or(Self::DEFAULT_LIMIT),
        )
    }

    /// A write succeeded; the run of skips is broken.
    pub fn record_success(&mut self) {
        self.consecutive = 0;
    }

    /// A permanent failure. Returns true when this one is over the limit, and the
    /// caller should halt instead of skipping it.
    pub fn record_skip(&mut self) -> bool {
        self.consecutive = self.consecutive.saturating_add(1);
        self.consecutive > self.limit
    }

    pub fn consecutive(&self) -> u32 {
        self.consecutive
    }
}

/// How long [`halt`] keeps the process alive before exiting, from
/// `INDEXER_HALT_HOLD_SECS` (default 30). Two 15s scrapes, so Prometheus sees the
/// `*_halted` gauge at 1 before the process, and the gauge, disappear.
pub fn halt_hold() -> Duration {
    Duration::from_secs(
        std::env::var("INDEXER_HALT_HOLD_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(30),
    )
}

/// Wait out [`halt_hold`], then exit with status 1 so Kubernetes restarts the pod
/// from the last committed offset. The caller has already logged the halt and set
/// its `*_halted` gauge, and must not have committed the failed message.
pub async fn halt() -> ! {
    tokio::time::sleep(halt_hold()).await;
    std::process::exit(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::HashMap;

    /// A writer that fails with the scripted errors in order, then succeeds.
    struct ScriptedWriter {
        failures: Vec<ErrorClass>,
        calls: Cell<usize>,
    }

    impl ScriptedWriter {
        fn new(failures: Vec<ErrorClass>) -> Self {
            Self {
                failures,
                calls: Cell::new(0),
            }
        }

        async fn write(&self) -> Result<&'static str, ErrorClass> {
            let n = self.calls.get();
            self.calls.set(n + 1);
            match self.failures.get(n) {
                Some(class) => Err(*class),
                None => Ok("written"),
            }
        }
    }

    fn policy(max_attempts: u32) -> RetryPolicy {
        RetryPolicy {
            max_attempts,
            initial_backoff: Duration::from_secs(2),
            max_backoff: Duration::from_secs(30),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn transient_failures_are_retried_until_the_write_lands() {
        let writer = ScriptedWriter::new(vec![ErrorClass::Transient, ErrorClass::Transient]);
        let mut delays = Vec::new();
        let outcome = run(
            &policy(5),
            |class: &ErrorClass| *class,
            |attempt, _, delay| delays.push((attempt, delay)),
            || writer.write(),
        )
        .await;
        assert!(matches!(outcome, Outcome::Written("written")));
        assert_eq!(writer.calls.get(), 3);
        assert_eq!(
            delays,
            vec![(1, Duration::from_secs(2)), (2, Duration::from_secs(4))]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_persistent_transient_failure_halts_after_the_last_attempt() {
        let writer = ScriptedWriter::new(vec![ErrorClass::Transient; 10]);
        let mut retries = 0;
        let outcome = run(
            &policy(4),
            |class: &ErrorClass| *class,
            |_, _, _| retries += 1,
            || writer.write(),
        )
        .await;
        match outcome {
            Outcome::Halt { error, attempts } => {
                assert_eq!(error, ErrorClass::Transient);
                assert_eq!(attempts, 4);
            }
            other => panic!("expected Halt, got {other:?}"),
        }
        assert_eq!(writer.calls.get(), 4, "never tried past max_attempts");
        assert_eq!(retries, 3, "one retry between each pair of attempts");
    }

    #[tokio::test(start_paused = true)]
    async fn a_permanent_failure_is_skipped_without_retrying() {
        let writer = ScriptedWriter::new(vec![ErrorClass::Permanent]);
        let outcome = run(
            &policy(5),
            |class: &ErrorClass| *class,
            |_, _, _| panic!("a permanent error must not be retried"),
            || writer.write(),
        )
        .await;
        assert!(matches!(outcome, Outcome::Skip(ErrorClass::Permanent)));
        assert_eq!(writer.calls.get(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_permanent_failure_after_transient_ones_is_still_skipped() {
        let writer = ScriptedWriter::new(vec![ErrorClass::Transient, ErrorClass::Permanent]);
        let outcome = run(
            &policy(5),
            |class: &ErrorClass| *class,
            |_, _, _| {},
            || writer.write(),
        )
        .await;
        assert!(matches!(outcome, Outcome::Skip(ErrorClass::Permanent)));
        assert_eq!(writer.calls.get(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn one_attempt_means_halt_on_the_first_transient_failure() {
        let writer = ScriptedWriter::new(vec![ErrorClass::Transient]);
        let outcome = run(
            &policy(1),
            |class: &ErrorClass| *class,
            |_, _, _| panic!("no retries with max_attempts = 1"),
            || writer.write(),
        )
        .await;
        assert!(matches!(outcome, Outcome::Halt { attempts: 1, .. }));
    }

    #[test]
    fn backoff_doubles_and_is_capped() {
        let p = policy(10);
        let delays: Vec<u64> = (1..=6).map(|a| p.backoff(a).as_secs()).collect();
        assert_eq!(delays, vec![2, 4, 8, 16, 30, 30]);
        // No overflow however large the attempt number.
        assert_eq!(p.backoff(u32::MAX), Duration::from_secs(30));
    }

    #[test]
    fn default_policy_waits_tens_of_seconds_before_halting() {
        let p = RetryPolicy::default();
        let total: Duration = (1..p.max_attempts).map(|a| p.backoff(a)).sum();
        assert_eq!(total, Duration::from_secs(30));
        assert_eq!(
            p.decide(p.max_attempts, ErrorClass::Transient),
            Decision::Halt
        );
    }

    #[test]
    fn decide_never_retries_a_permanent_error() {
        let p = policy(5);
        assert_eq!(p.decide(1, ErrorClass::Permanent), Decision::Skip);
        assert_eq!(
            p.decide(1, ErrorClass::Transient),
            Decision::Retry(Duration::from_secs(2))
        );
        assert_eq!(p.decide(5, ErrorClass::Transient), Decision::Halt);
        // max_attempts = 0 is treated as 1, never as "retry forever".
        assert_eq!(policy(0).decide(1, ErrorClass::Transient), Decision::Halt);
    }

    #[test]
    fn policy_reads_its_environment_and_ignores_bad_values() {
        let env: HashMap<&str, &str> = HashMap::from([
            ("INDEXER_WRITE_MAX_ATTEMPTS", "3"),
            ("INDEXER_WRITE_BACKOFF_MS", " 500 "),
            ("INDEXER_WRITE_BACKOFF_MAX_MS", "4000"),
        ]);
        let p = RetryPolicy::from_lookup(|k| env.get(k).map(|v| v.to_string()));
        assert_eq!(p.max_attempts, 3);
        assert_eq!(p.initial_backoff, Duration::from_millis(500));
        assert_eq!(p.max_backoff, Duration::from_millis(4000));

        let bad: HashMap<&str, &str> = HashMap::from([
            ("INDEXER_WRITE_MAX_ATTEMPTS", "0"),
            ("INDEXER_WRITE_BACKOFF_MS", "banana"),
            ("INDEXER_WRITE_BACKOFF_MAX_MS", "-1"),
        ]);
        let p = RetryPolicy::from_lookup(|k| bad.get(k).map(|v| v.to_string()));
        assert_eq!(p, RetryPolicy::default());
    }

    #[test]
    fn max_backoff_is_never_below_the_initial_backoff() {
        let env: HashMap<&str, &str> = HashMap::from([
            ("INDEXER_WRITE_BACKOFF_MS", "5000"),
            ("INDEXER_WRITE_BACKOFF_MAX_MS", "1000"),
        ]);
        let p = RetryPolicy::from_lookup(|k| env.get(k).map(|v| v.to_string()));
        assert_eq!(p.backoff(1), Duration::from_millis(5000));
    }

    #[test]
    fn only_data_sqlstates_are_permanent() {
        for code in [
            "22001", "22003", "22P02", "23502", "23503", "23505", "23514",
        ] {
            assert_eq!(classify_sqlstate(code), ErrorClass::Permanent, "{code}");
        }
        for code in [
            "08006", // connection_failure
            "40001", // serialization_failure
            "40P01", // deadlock_detected
            "53300", // too_many_connections
            "55P03", // lock_not_available
            "57014", // query_canceled (statement_timeout)
            "57P01", // admin_shutdown
            "42P01", // undefined_table: a bad migration, not a bad message
            "42703", // undefined_column
            "XX000", // internal_error
        ] {
            assert_eq!(classify_sqlstate(code), ErrorClass::Transient, "{code}");
        }
    }

    #[test]
    fn non_database_sqlx_errors_are_transient() {
        assert_eq!(
            classify_sqlx(&sqlx::Error::PoolTimedOut),
            ErrorClass::Transient
        );
        assert_eq!(
            classify_sqlx(&sqlx::Error::PoolClosed),
            ErrorClass::Transient
        );
        assert_eq!(
            classify_sqlx(&sqlx::Error::Io(std::io::Error::other("reset"))),
            ErrorClass::Transient
        );
        // A missing row is not proof the message is bad: halting is the safe side.
        assert_eq!(
            classify_sqlx(&sqlx::Error::RowNotFound),
            ErrorClass::Transient
        );
    }

    /// Items 1..=5; a test writes the first `len` of them.
    const ALL: [u32; 5] = [1, 2, 3, 4, 5];

    /// A batch writer that fails any slice containing a poison item, and fails
    /// transiently a scripted number of times first.
    struct BatchWriter {
        poison: Vec<u32>,
        down: Vec<u32>,
        transient_left: Cell<u32>,
        calls: Cell<usize>,
        written: std::cell::RefCell<Vec<u32>>,
    }

    impl BatchWriter {
        fn new(poison: &[u32], down: &[u32], transient_failures: u32) -> Self {
            Self {
                poison: poison.to_vec(),
                down: down.to_vec(),
                transient_left: Cell::new(transient_failures),
                calls: Cell::new(0),
                written: std::cell::RefCell::new(Vec::new()),
            }
        }

        async fn write(&self, items: &[u32]) -> Result<(), ErrorClass> {
            self.calls.set(self.calls.get() + 1);
            if self.transient_left.get() > 0 {
                self.transient_left.set(self.transient_left.get() - 1);
                return Err(ErrorClass::Transient);
            }
            if items.iter().any(|i| self.down.contains(i)) {
                return Err(ErrorClass::Transient);
            }
            if items.iter().any(|i| self.poison.contains(i)) {
                return Err(ErrorClass::Permanent);
            }
            self.written.borrow_mut().extend_from_slice(items);
            Ok(())
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_clean_batch_is_written_in_one_call() {
        let writer = BatchWriter::new(&[], &[], 1);
        let mut guard = SkipGuard::new(10);
        let outcome = write_batch(
            3,
            &policy(5),
            |c: &ErrorClass| *c,
            &mut guard,
            |_, _, _| {},
            true,
            |r| writer.write(&ALL[r]),
        )
        .await;
        match outcome {
            BatchOutcome::Done(report) => {
                assert_eq!(report.written, 3);
                assert!(report.skipped.is_empty());
            }
            other => panic!("expected Done, got {other:?}"),
        }
        assert_eq!(
            writer.calls.get(),
            2,
            "one transient failure, then the batch"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_poison_item_is_isolated_and_its_neighbours_are_written() {
        let writer = BatchWriter::new(&[2], &[], 0);
        let mut guard = SkipGuard::new(10);
        let outcome = write_batch(
            3,
            &policy(5),
            |c: &ErrorClass| *c,
            &mut guard,
            |_, _, _| {},
            true,
            |r| writer.write(&ALL[r]),
        )
        .await;
        match outcome {
            BatchOutcome::Done(report) => {
                assert_eq!(report.written, 2);
                assert_eq!(report.skipped.len(), 1);
                assert_eq!(report.skipped[0].0, 1, "the poison item's index");
            }
            other => panic!("expected Done, got {other:?}"),
        }
        assert_eq!(*writer.written.borrow(), vec![1, 3]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_transient_failure_mid_batch_halts_at_that_item() {
        // Item 3 hits a database that stays down: 1 and 2 are written, 3 is not,
        // and nothing after it is attempted.
        let writer = BatchWriter::new(&[], &[3], 0);
        let mut guard = SkipGuard::new(10);
        let outcome = write_batch(
            4,
            &policy(3),
            |c: &ErrorClass| *c,
            &mut guard,
            |_, _, _| {},
            false,
            |r| writer.write(&ALL[r]),
        )
        .await;
        match outcome {
            BatchOutcome::Halt {
                index,
                attempts,
                report,
                ..
            } => {
                assert_eq!(index, 2, "halted on the third item");
                assert_eq!(attempts, 3);
                assert_eq!(report.written, 2);
            }
            other => panic!("expected Halt, got {other:?}"),
        }
        assert_eq!(
            *writer.written.borrow(),
            vec![1, 2],
            "item 4 never attempted"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_whole_batch_that_stays_down_halts_before_writing_anything() {
        let writer = BatchWriter::new(&[], &[], 100);
        let mut guard = SkipGuard::new(10);
        let outcome = write_batch(
            2,
            &policy(5),
            |c: &ErrorClass| *c,
            &mut guard,
            |_, _, _| {},
            true,
            |r| writer.write(&ALL[r]),
        )
        .await;
        assert!(matches!(outcome, BatchOutcome::Halt { index: 0, .. }));
        assert!(writer.written.borrow().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_batch_of_nothing_but_poison_halts_as_systemic() {
        let writer = BatchWriter::new(&[1, 2, 3, 4, 5], &[], 0);
        let mut guard = SkipGuard::new(2);
        let outcome = write_batch(
            5,
            &policy(5),
            |c: &ErrorClass| *c,
            &mut guard,
            |_, _, _| {},
            true,
            |r| writer.write(&ALL[r]),
        )
        .await;
        match outcome {
            BatchOutcome::Halt { index, report, .. } => {
                assert_eq!(
                    index, 2,
                    "the third permanent failure in a row trips a limit of 2"
                );
                assert_eq!(report.skipped.len(), 2);
            }
            other => panic!("expected Halt, got {other:?}"),
        }
    }

    #[test]
    fn skip_guard_halts_on_a_run_of_permanent_failures() {
        let mut guard = SkipGuard::new(3);
        assert!(!guard.record_skip());
        assert!(!guard.record_skip());
        assert!(!guard.record_skip());
        assert!(guard.record_skip(), "the fourth in a row is systemic");

        guard.record_success();
        assert_eq!(guard.consecutive(), 0);
        assert!(!guard.record_skip(), "a success resets the run");
    }
}
