//! Replays the `notification_outbox` rows of a time window into geo-db's work class (RUNBOOK
//! section 11). Dry run by default; `--apply` writes. Run it in a notification-indexer pod, which
//! already has `DATABASE_URL`, the geodb client, the work key and `GEODB_OUTBOX_MIRROR=1`.
//!
//! ```text
//! outbox-replay --from <rfc3339> --to <rfc3339> [--apply] [--batch N] [--rate N] [--max-units N]
//! ```
//! `--to` is exclusive. Postgres is opened read-only. A dry run asks geo-db which of the window's
//! keys it already holds with the read-only `has` verb when the geodb client and node have it, and
//! otherwise reports `present unknown`.

use std::process::ExitCode;

use chrono::{DateTime, Utc};
use notification_indexer::outbox_replay::{
    interval_for, read_only_pool, run, Options, PgSource, Probe, BATCH_DEFAULT, BATCH_MAX,
    RATE_DEFAULT, RATE_MAX,
};

struct Args {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    apply: bool,
    batch: usize,
    rate: u32,
    max_units: Option<u64>,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let (mut from, mut to, mut apply, mut dry) = (None, None, false, false);
    let (mut batch, mut rate, mut max_units) = (BATCH_DEFAULT, RATE_DEFAULT, None);
    let mut it = args.iter();
    let time = |flag: &str, v: Option<&String>| -> Result<DateTime<Utc>, String> {
        let v = v.ok_or(format!("{flag} needs a value"))?;
        DateTime::parse_from_rfc3339(v)
            .map(|t| t.with_timezone(&Utc))
            .map_err(|e| format!("{flag} {v:?}: {e}"))
    };
    let num = |flag: &str, v: Option<&String>| -> Result<u64, String> {
        let v = v.ok_or(format!("{flag} needs a value"))?;
        v.parse()
            .map_err(|_| format!("{flag} {v:?} is not a number"))
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--from" => from = Some(time("--from", it.next())?),
            "--to" => to = Some(time("--to", it.next())?),
            "--apply" => apply = true,
            "--dry-run" => dry = true,
            "--batch" => batch = num("--batch", it.next())? as usize,
            "--rate" => rate = num("--rate", it.next())?.min(u64::from(u32::MAX)) as u32,
            "--max-units" => max_units = Some(num("--max-units", it.next())?),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    if apply && dry {
        return Err("--apply and --dry-run contradict each other".into());
    }
    let (from, to) = (
        from.ok_or("--from is required")?,
        to.ok_or("--to is required")?,
    );
    if from >= to {
        return Err("--from must be before --to".into());
    }
    if !(1..=BATCH_MAX).contains(&batch) {
        return Err(format!("--batch must be 1..={BATCH_MAX}"));
    }
    if !(1..=RATE_MAX).contains(&rate) {
        return Err(format!("--rate must be 1..={RATE_MAX}"));
    }
    Ok(Args {
        from,
        to,
        apply,
        batch,
        rate,
        max_units,
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("outbox-replay: {e}");
            return ExitCode::from(2);
        }
    };
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("outbox-replay: DATABASE_URL is not set");
        return ExitCode::from(2);
    };
    // An apply needs the geo-db connection. A dry run uses it only for the read-only `has` probe,
    // so without it the dry run still works and says `present unknown`.
    let mut enterer = geodb_outbox::Config::from_env().map(geodb_outbox::Enterer::new);
    if args.apply && enterer.is_none() {
        eprintln!(
            "outbox-replay: --apply needs GEODB_OUTBOX_MIRROR=1, GEODB_OUTBOX_KEY_PATH and GEODB_BINDING_PATH"
        );
        return ExitCode::from(2);
    }
    let pool = match read_only_pool(&url).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("outbox-replay: postgres: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut src = PgSource {
        pool,
        from: args.from,
        to: args.to,
    };
    let opts = Options {
        batch: args.batch,
        interval: interval_for(args.rate),
        max_units: args.max_units,
        apply: args.apply,
    };
    println!(
        "outbox-replay {} window [{}, {}) batch {} rate {}/s",
        if args.apply { "APPLY" } else { "DRY RUN" },
        args.from.to_rfc3339(),
        args.to.to_rfc3339(),
        args.batch,
        args.rate
    );
    let result = match enterer.as_mut() {
        Some(e) => run(&mut src, e, &opts).await,
        None => run(&mut src, &mut NoSink, &opts).await,
    };
    match result {
        Ok(r) => {
            print_report(&r, args.apply);
            ExitCode::SUCCESS
        }
        Err(s) => {
            print_report(&s.report, args.apply);
            eprintln!("outbox-replay: stopped: {}", s.reason);
            ExitCode::FAILURE
        }
    }
}

fn print_report(r: &notification_indexer::outbox_replay::Report, apply: bool) {
    println!("scanned     {}", r.scanned);
    println!(
        "unencodable {} (skipped, as the live mirror skips them)",
        r.unencodable
    );
    if apply {
        println!("present     {} (geo-db already held the key)", r.present);
        println!("entered     {}", r.entered);
    } else if r.probe == Probe::Used {
        println!("present     {} (geo-db already holds the key)", r.held);
        println!("missing     {} (an apply enters these)", r.missing);
        println!("would-enter {} (the missing ones)", r.missing);
    } else {
        let why = match &r.probe {
            Probe::Unavailable(why) => format!("the has probe is unavailable: {why}"),
            _ => "nothing to probe".to_string(),
        };
        println!("present     unknown ({why}; --apply reports it)");
        println!(
            "would-enter {} (at most; an apply skips the ones geo-db holds)",
            r.would_enter
        );
    }
    if let Some((at, id)) = r.last {
        println!("last        {} {id}", at.to_rfc3339());
    }
}

/// A dry run with no geo-db connection: it never enters, and `has` is its default, unsupported.
struct NoSink;
impl notification_indexer::outbox_replay::Sink for NoSink {
    async fn enter(
        &mut self,
        _: &geodb_outbox::Unit,
    ) -> Result<geodb_outbox::Entered, geodb_outbox::MirrorError> {
        unreachable!("a dry run does not enter")
    }
}
