//! Replays the `notification_outbox` rows of a time window into geo-db's work class (RUNBOOK
//! section 11). Dry run by default; `--apply` writes. Run it in a notification-indexer pod, which
//! already has `DATABASE_URL`, the geodb client, the work key and `GEODB_OUTBOX_MIRROR=1`.
//!
//! ```text
//! outbox-replay --from <rfc3339> --to <rfc3339> [--apply] [--batch N] [--rate N] [--max-units N]
//! ```
//! `--to` is exclusive. Postgres is opened read-only.

use std::process::ExitCode;

use chrono::{DateTime, Utc};
use notification_indexer::outbox_replay::{
    interval_for, run, Cursor, Options, Row, RowSource, BATCH_DEFAULT, BATCH_MAX, RATE_DEFAULT,
    RATE_MAX,
};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row as _};
use uuid::Uuid;

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

struct Pg {
    pool: PgPool,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
}

impl RowSource for Pg {
    async fn page(&mut self, after: Option<Cursor>, limit: usize) -> Result<Vec<Row>, String> {
        let (at, id) = after.unzip();
        let rows = sqlx::query(
            r#"
            SELECT o.created_at, o.id, o.idempotency_key, o.event_type, o.payload,
                   ARRAY(SELECT d.webhook_id FROM notification_deliveries d
                         WHERE d.outbox_id = o.id ORDER BY d.webhook_id) AS webhooks
            FROM notification_outbox o
            WHERE o.created_at >= $1 AND o.created_at < $2
              AND ($3::timestamptz IS NULL OR (o.created_at, o.id) > ($3, $4))
            ORDER BY o.created_at, o.id
            LIMIT $5
            "#,
        )
        .bind(self.from)
        .bind(self.to)
        .bind(at)
        .bind(id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter()
            .map(|r| {
                Ok(Row {
                    created_at: r.try_get("created_at")?,
                    id: r.try_get("id")?,
                    idempotency_key: r.try_get("idempotency_key")?,
                    event_type: r.try_get("event_type")?,
                    payload: r.try_get("payload")?,
                    webhooks: r.try_get::<Vec<Uuid>, _>("webhooks")?,
                })
            })
            .collect::<Result<_, sqlx::Error>>()
            .map_err(|e| e.to_string())
    }
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
    let mut enterer = None;
    if args.apply {
        match geodb_outbox::Config::from_env() {
            Some(cfg) => enterer = Some(geodb_outbox::Enterer::new(cfg)),
            None => {
                eprintln!(
                    "outbox-replay: --apply needs GEODB_OUTBOX_MIRROR=1, GEODB_OUTBOX_KEY_PATH and GEODB_BINDING_PATH"
                );
                return ExitCode::from(2);
            }
        }
    }
    let pool = match PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|c, _| {
            Box::pin(async move {
                sqlx::query("SET default_transaction_read_only = on")
                    .execute(c)
                    .await
                    .map(|_| ())
            })
        })
        .connect(&url)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("outbox-replay: postgres: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut src = Pg {
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
    } else {
        println!("present     unknown (a dry run never touches geo-db; --apply reports it)");
        println!(
            "would-enter {} (at most; an apply skips the ones geo-db holds)",
            r.would_enter
        );
    }
    if let Some((at, id)) = r.last {
        println!("last        {} {id}", at.to_rfc3339());
    }
}

/// A dry run never calls its sink.
struct NoSink;
impl notification_indexer::outbox_replay::Sink for NoSink {
    async fn enter(
        &mut self,
        _: &geodb_outbox::Unit,
    ) -> Result<geodb_outbox::Entered, geodb_outbox::MirrorError> {
        unreachable!("a dry run does not enter")
    }
}
