//! Reconciles geo-db collection 28 (`user_topic_signals`) with Postgres, once.
//!
//! `not_interested_sync` used to delete and re-insert a user's rows without mirroring the delete,
//! so rows Postgres no longer has stayed in geo-db and inflated users' private `weights` (gaia#1501
//! stops new ones). This reads every user's group back from geo-db, compares it with Postgres, and
//! plans a delete for each key only geo-db has and a put for each row that is missing or differs.
//! The plan is `ranking_indexer::user_topic_signals_reconcile`; why and how it judges are there.
//!
//! DRY RUN BY DEFAULT: it reads both stores, prints counts (never an id) and writes nothing.
//! `--apply` is the only thing that writes. With `--apply` it plans the whole table first and
//! refuses to continue if the plan looks like a broken read (it would delete more than
//! `--max-delete-fraction`, default 0.2, of what geo-db holds, or Postgres returned nothing) or
//! changes more than `--max-changes` (default 200000) rows, unless `--allow-mass-change`; then it
//! walks the table again, and for each page scans geo-db FIRST and reads Postgres second, so a row
//! written between the two reads is never deleted, and applies that page at once.
//!
//! Needs `DATABASE_URL`, `GEODB_DERIVED_MIRROR=1`, `GEODB_DERIVED_MIRROR_PRIVATE=1` and the
//! geodb-client wiring (`GEODB_CLIENT_BIN`, `GEODB_KEY_PATH`, `GEODB_BINDING_PATH`), as
//! `private_mirror_snapshot` does. Exit codes: 0 done (dry run: nothing to change), 1 could not read
//! or finish, 2 dry run found changes to make.
//!
//! Usage: `user_topic_signals_reconcile [--apply] [--page-users N] [--sample N] [--scan-pages N]
//!   [--max-delete-fraction F] [--max-changes N] [--allow-mass-change]`

use std::env;

use geodb_cursor::derived::{DerivedMirror, BATCH_MAX};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tracing::{error, info, warn};
use uuid::Uuid;

use ranking_indexer::error::IndexerError;
use ranking_indexer::external_interest_mirror::private_enabled;
use ranking_indexer::private_snapshot::ScanPipe;
use ranking_indexer::user_topic_signals_mirror::{self as topic, Signal};
use ranking_indexer::user_topic_signals_reconcile::{
    looks_broken, page_rows_sql, plan_user, tag_name, want_by_user, Counts, UserPlan, USERS_SQL,
};

#[derive(Debug, Clone, PartialEq)]
struct Options {
    apply: bool,
    page_users: i64,
    sample: usize,
    scan_pages: usize,
    max_delete_fraction: f64,
    max_changes: u64,
    allow_mass_change: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            apply: false,
            page_users: 200,
            sample: 5,
            scan_pages: 16,
            max_delete_fraction: 0.2,
            max_changes: 200_000,
            allow_mass_change: false,
        }
    }
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Options, IndexerError> {
    let mut o = Options::default();
    let mut args = args;
    fn num<T: std::str::FromStr>(flag: &str, v: Option<String>) -> Result<T, IndexerError> {
        v.and_then(|v| v.parse().ok())
            .ok_or_else(|| IndexerError::Config(format!("{flag} needs a number")))
    }
    while let Some(a) = args.next() {
        match a.as_str() {
            "--apply" => o.apply = true,
            "--allow-mass-change" => o.allow_mass_change = true,
            "--page-users" => o.page_users = num(&a, args.next())?,
            "--sample" => o.sample = num(&a, args.next())?,
            "--scan-pages" => o.scan_pages = num(&a, args.next())?,
            "--max-delete-fraction" => o.max_delete_fraction = num(&a, args.next())?,
            "--max-changes" => o.max_changes = num(&a, args.next())?,
            other => return Err(IndexerError::Config(format!("unknown argument {other}"))),
        }
    }
    if o.page_users < 1 || o.scan_pages < 1 || !(0.0..=1.0).contains(&o.max_delete_fraction) {
        return Err(IndexerError::Config(
            "--page-users and --scan-pages must be at least 1, --max-delete-fraction in 0..=1"
                .into(),
        ));
    }
    Ok(o)
}

fn scan_pipe() -> Result<ScanPipe, String> {
    let cfg = geodb_cursor::Config::from_vars_for("GEODB_DERIVED_MIRROR", |k| env::var(k).ok())
        .ok_or("GEODB_DERIVED_MIRROR is off or the key and binding paths are unset")?;
    ScanPipe::spawn(&cfg.program, &cfg.args)
}

/// One walk of the table. With `mirror` it applies each page; without, it only counts.
async fn walk(
    pool: &PgPool,
    pipe: &mut ScanPipe,
    mirror: Option<&DerivedMirror>,
    o: &Options,
) -> Result<(Counts, Vec<String>), String> {
    let rows_sql = page_rows_sql();
    let mut counts = Counts::default();
    let mut sample: Vec<String> = Vec::new();
    let mut after = Uuid::nil();
    let mut scan_errors = 0u32;
    loop {
        let users: Vec<(Uuid,)> = sqlx::query_as(USERS_SQL)
            .bind(after)
            .bind(o.page_users)
            .fetch_all(pool)
            .await
            .map_err(|e| format!("user page read failed: {e}"))?;
        let Some(last) = users.last() else { break };
        after = last.0;
        let ids: Vec<Uuid> = users.iter().map(|u| u.0).collect();

        // geo-db first, Postgres second: anything geo-db holds that was written after this scan
        // is not in the plan, and anything Postgres changes after its read is the next run's.
        let mut held = Vec::with_capacity(ids.len());
        for id in &ids {
            match pipe.scan_pages(topic::COLLECTION, id.as_u128(), o.scan_pages) {
                Ok(h) => held.push(h),
                Err(e) => {
                    scan_errors += 1;
                    if scan_errors == 1 {
                        error!(error = %e, "reconcile: scan failed");
                    }
                    if scan_errors >= 5 {
                        return Err("five scans failed".into());
                    }
                    held.push((Vec::new(), true));
                }
            }
        }
        let signals: Vec<Signal> = sqlx::query_as(sqlx::AssertSqlSafe(rows_sql.clone()))
            .bind(topic::refit_since())
            .bind(&ids)
            .fetch_all(pool)
            .await
            .map_err(|e| format!("row read failed: {e}"))?;
        let want = want_by_user(&signals);

        let (mut puts, mut deletes) = (Vec::new(), Vec::new());
        for (id, (rows, more)) in ids.iter().zip(held) {
            let w = want.get(&id.as_u128()).map(Vec::as_slice).unwrap_or(&[]);
            let mut p = plan_user(w, &rows, more);
            counts.add(w.len(), rows.len(), &p);
            note_sample(&mut sample, &p, o.sample);
            puts.append(&mut p.puts);
            deletes.append(&mut p.deletes);
        }
        if let Some(m) = mirror {
            for chunk in puts.chunks(BATCH_MAX) {
                let sent = m.put_counted("user_topic_signals", chunk).await;
                if sent != chunk.len() {
                    return Err(format!(
                        "geo-db acknowledged {sent} of {} puts; stopping",
                        chunk.len()
                    ));
                }
            }
            for chunk in deletes.chunks(BATCH_MAX) {
                m.delete("user_topic_signals", topic::COLLECTION, chunk)
                    .await;
            }
        }
        if scan_errors > 0 {
            warn!(
                scan_errors,
                "reconcile: some groups were not read this page"
            );
        }
    }
    Ok((counts, sample))
}

/// A line per planned change with no id in it: what kind, and what differs.
fn note_sample(sample: &mut Vec<String>, p: &UserPlan, max: usize) {
    for t in &p.delete_tags {
        if sample.len() < max {
            sample.push(format!("delete stale {} row", tag_name(*t)));
        }
    }
    if p.missing > 0 && sample.len() < max {
        sample.push(format!("put {} row(s) geo-db lacks", p.missing));
    }
    if !p.drifted.is_empty() && sample.len() < max {
        let f: Vec<&str> = p.drifted.iter().map(|d| d.label()).collect();
        sample.push(format!("put drifted row: {}", f.join(",")));
    }
}

fn report(label: &str, c: &Counts, sample: &[String]) {
    info!(
        label,
        users = c.users,
        users_with_changes = c.users_with_changes,
        postgres_rows = c.pg_rows,
        geodb_rows = c.held_rows,
        matched = c.matched,
        put_missing = c.missing,
        put_drifted = c.drifted_rows,
        delete_stale = c.stale,
        foreign_keys_left_alone = c.foreign,
        partial_groups = c.partial_groups,
        "reconcile: counts"
    );
    info!(label, drift_by_field = ?c.drift_by_field, stale_by_kind = ?c.stale_by_kind, "reconcile: breakdown");
    for s in sample {
        info!(label, "reconcile: sample: {s}");
    }
}

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt().with_env_filter("info").init();
    let o = parse_args(env::args().skip(1))?;
    if !private_enabled() {
        return Err(IndexerError::Config(
            "GEODB_DERIVED_MIRROR_PRIVATE is not 1".into(),
        ));
    }
    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let mirror = if o.apply {
        Some(DerivedMirror::from_env().ok_or_else(|| {
            IndexerError::Config("geo-db derived mirror is off or unconfigured".into())
        })?)
    } else {
        None
    };
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .map_err(|e| IndexerError::Config(format!("database connect failed: {e}")))?;
    let mut pipe = scan_pipe().map_err(IndexerError::Config)?;
    info!(apply = o.apply, "reconcile: starting (collection 28)");

    let (plan, sample) = match walk(&pool, &mut pipe, None, &o).await {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "reconcile: read failed");
            std::process::exit(1);
        }
    };
    report(if o.apply { "plan" } else { "dry run" }, &plan, &sample);
    if !o.apply {
        info!("reconcile: dry run, nothing written; pass --apply to write");
        std::process::exit(if plan.changes() == 0 { 0 } else { 2 });
    }
    if !o.allow_mass_change
        && (looks_broken(&plan, o.max_delete_fraction) || plan.changes() > o.max_changes)
    {
        error!(
            changes = plan.changes(),
            "reconcile: the plan looks like a broken read or is larger than --max-changes; \
             refusing (--allow-mass-change overrides)"
        );
        std::process::exit(1);
    }
    let (done, _) = match walk(&pool, &mut pipe, mirror.as_ref(), &o).await {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "reconcile: apply stopped; re-running is safe");
            std::process::exit(1);
        }
    };
    report("applied", &done, &[]);
    info!("reconcile: done; run it again without --apply and expect nothing to change");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> impl Iterator<Item = String> {
        a.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn it_is_a_dry_run_unless_apply_is_given() {
        assert!(!parse_args(args(&[])).unwrap().apply);
        assert!(parse_args(args(&["--apply"])).unwrap().apply);
        assert!(
            !parse_args(args(&["--page-users", "50", "--sample", "9"]))
                .unwrap()
                .apply
        );
    }

    #[test]
    fn numbers_are_read_and_bad_input_is_refused() {
        let o = parse_args(args(&[
            "--page-users",
            "50",
            "--max-delete-fraction",
            "0.5",
        ]))
        .unwrap();
        assert_eq!((o.page_users, o.max_delete_fraction), (50, 0.5));
        assert!(parse_args(args(&["--page-users"])).is_err());
        assert!(parse_args(args(&["--page-users", "0"])).is_err());
        assert!(parse_args(args(&["--max-delete-fraction", "2"])).is_err());
        assert!(parse_args(args(&["--appply"])).is_err());
    }
}
