//! Syncs the test-account exclusion list from analytics into gaia (GEO-3141).
//!
//! gaia knows a voter only as their Geo personal space (`user_votes.user_id`); which accounts are
//! test accounts is known only to analytics (Privy labels, emails). Analytics publishes the list
//! as ClickHouse view `analytics.gaia_account_exclusions` (analytics migration 106): one row per
//! (wallet_address_hash, reason), where the hash is
//! `'sha256:' || hex(sha256('wallet:' || lower(address)))` of a Privy wallet. A Geo personal
//! space's on-chain address IS the owner's Privy embedded wallet (EIP-7702; the v2 space registry
//! keys on the Privy EOA, and geogenesis resolves a user's personal space by
//! `spaces.address ILIKE <privy wallet>`), so hashing `spaces.address` the same way places each
//! row on a personal space. No email, name or Privy id ever reaches gaia.
//!
//! The list then replaces the analytics-sourced rows of `account_exclusions` in one call
//! (`replace_account_exclusions`, migration 0101), and the hourly `refresh_account_weights` gives
//! those voters weight 0. Rows from other sources (a manual exclusion) are untouched.
//!
//! Guards, each of which exits non-zero WITHOUT writing:
//!   * analytics returned no rows (an outage or a broken view must not un-exclude everyone; the
//!     SQL function refuses an empty list too);
//!   * rows came back but none matched a personal space (the hashing no longer agrees);
//!   * more than EXCLUSION_SYNC_MAX_EXCLUDED spaces matched (default 500).
//!
//! Dry run (`--dry-run`, or EXCLUSION_SYNC_DRY_RUN=1) prints what it would write and the effect
//! on the voter count, and writes nothing. Voters are counted as the 2 Oct audit counted them
//! (stance votes on claims; `--all-voters` for every kind), and `--as-of 2026-10-02` counts them
//! as of that date (the GEO-3141 acceptance check: 159 voters down to 128). `--list-kept` also prints every voter
//! who stays, by their public Geo name, to check the team is kept. Names come from gaia's public
//! data, never from analytics.
//!
//! Input: ClickHouse over HTTPS (CLICKHOUSE_URL, CLICKHOUSE_USER, CLICKHOUSE_PASSWORD), or
//! `--input <file>` with the view's rows as JSONEachRow (e.g. exported from the ClickHouse console)
//! for an offline dry run.
//!
//! Usage:
//!   DATABASE_URL=... CLICKHOUSE_URL=... CLICKHOUSE_USER=... CLICKHOUSE_PASSWORD=... \
//!     account_exclusions_sync [--dry-run] [--as-of YYYY-MM-DD] [--list-kept] [--input rows.jsonl]

use std::collections::BTreeMap;
use std::env;
use std::time::Duration;

use serde::Deserialize;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tracing::{info, warn};

use ranking_indexer::error::IndexerError;

const VIEW_QUERY: &str =
    "SELECT wallet_address_hash, reason FROM analytics.gaia_account_exclusions FORMAT JSONEachRow";
const DEFAULT_MAX_EXCLUDED: usize = 500;
const SOURCE: &str = "analytics";

/// identity_hash('wallet', '0x929e5195f039E0becB79B039339077FA17183064') as analytics computes it
/// (crates/analytics-ingest/src/identity.rs). Checked against Postgres at startup, so a change to
/// either side's hashing fails loudly instead of silently matching nobody.
const KNOWN_ADDRESS: &str = "0x929e5195f039E0becB79B039339077FA17183064";
const KNOWN_HASH: &str = "sha256:645c48b3494f9f1fba91b84d2c1fa55d23cfa134be3e1e488803286c13bff73b";

/// gaia's side of the hash. Must stay identical to analytics' identity_hash("wallet", address).
const SPACE_ADDRESS_HASH_SQL: &str =
    "'sha256:' || encode(sha256(convert_to('wallet:' || lower(btrim(s.address)), 'UTF8')), 'hex')";

#[derive(Debug, Deserialize, Clone, PartialEq)]
struct Candidate {
    wallet_address_hash: Option<String>,
    reason: String,
}

#[derive(Debug, Default)]
struct Options {
    dry_run: bool,
    list_kept: bool,
    as_of: Option<String>,
    input: Option<String>,
    /// Count every voter, not just stance voters on claims (the 2 Oct audit's definition).
    all_voters: bool,
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Options, IndexerError> {
    let mut options = Options {
        dry_run: matches!(
            env::var("EXCLUSION_SYNC_DRY_RUN").as_deref(),
            Ok("1") | Ok("true") | Ok("yes")
        ),
        ..Options::default()
    };
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dry-run" => options.dry_run = true,
            "--list-kept" => options.list_kept = true,
            "--all-voters" => options.all_voters = true,
            "--as-of" => options.as_of = args.next(),
            "--input" => options.input = args.next(),
            other => {
                return Err(IndexerError::Config(format!(
                    "unknown argument {other}; usage: account_exclusions_sync [--dry-run] [--as-of YYYY-MM-DD] [--list-kept] [--all-voters] [--input rows.jsonl]"
                )))
            }
        }
    }
    Ok(options)
}

/// Parses the view's JSONEachRow output. Blank lines are skipped; a malformed line is an error,
/// never a silently shorter list.
fn parse_rows(body: &str) -> Result<Vec<Candidate>, IndexerError> {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_str::<Candidate>(line).map_err(|e| {
                IndexerError::Config(format!("unreadable exclusion row {line:?}: {e}"))
            })
        })
        .collect()
}

async fn fetch_from_clickhouse() -> Result<String, IndexerError> {
    let url = env::var("CLICKHOUSE_URL")
        .map_err(|_| IndexerError::Config("CLICKHOUSE_URL not set (or pass --input)".into()))?;
    let user = env::var("CLICKHOUSE_USER")
        .map_err(|_| IndexerError::Config("CLICKHOUSE_USER not set".into()))?;
    let password = env::var("CLICKHOUSE_PASSWORD")
        .map_err(|_| IndexerError::Config("CLICKHOUSE_PASSWORD not set".into()))?;
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| IndexerError::Config(format!("http client: {e}")))?
        .post(url.trim_end_matches('/').to_string() + "/")
        .header("X-ClickHouse-User", user)
        .header("X-ClickHouse-Key", password)
        .body(VIEW_QUERY)
        .send()
        .await
        .map_err(|e| IndexerError::Config(format!("ClickHouse request failed: {e}")))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| IndexerError::Config(format!("ClickHouse response unreadable: {e}")))?;
    if !status.is_success() {
        return Err(IndexerError::Config(format!(
            "ClickHouse returned {status}: {}",
            body.chars().take(500).collect::<String>()
        )));
    }
    Ok(body)
}

/// The personal spaces the candidates place, with every reason that applies, by space id.
async fn place_on_spaces(
    pool: &PgPool,
    candidates: &[Candidate],
) -> Result<BTreeMap<uuid::Uuid, String>, IndexerError> {
    let rows = serde_json::to_value(
        candidates
            .iter()
            .filter_map(|c| {
                c.wallet_address_hash.as_ref().map(|h| {
                    serde_json::json!({"wallet_address_hash": h.trim().to_ascii_lowercase(), "reason": c.reason})
                })
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|e| IndexerError::Config(e.to_string()))?;
    let sql = format!(
        "WITH c AS (SELECT * FROM jsonb_to_recordset($1::jsonb) AS x(wallet_address_hash text, reason text)) \
         SELECT s.id, string_agg(DISTINCT c.reason, ',' ORDER BY c.reason) \
         FROM public.spaces s JOIN c ON c.wallet_address_hash = {SPACE_ADDRESS_HASH_SQL} \
         WHERE s.type = 'Personal' GROUP BY s.id"
    );
    let placed: Vec<(uuid::Uuid, String)> = sqlx::query_as(&sql).bind(rows).fetch_all(pool).await?;
    Ok(placed.into_iter().collect())
}

#[derive(sqlx::FromRow)]
struct VoterRow {
    user_id: uuid::Uuid,
    name: Option<String>,
    votes: i64,
}

/// Voters by vote count. By default, as the 2 Oct 2026 audit counted them: accounts with a stance
/// vote (Agree/Disagree) on a Claim. `all_voters` counts every vote kind.
async fn voters(
    pool: &PgPool,
    as_of: &Option<String>,
    all_voters: bool,
) -> Result<Vec<VoterRow>, IndexerError> {
    let rows = sqlx::query_as::<_, VoterRow>(
        "SELECT uv.user_id, \
                (SELECT v.text FROM public.spaces s CROSS JOIN LATERAL public.spaces_page(s) p \
                 JOIN public.values v ON v.entity_id = p.id \
                  AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935'::uuid \
                 WHERE s.id = uv.user_id LIMIT 1) AS name, \
                count(*) AS votes \
         FROM public.user_votes uv \
         WHERE ($1::text IS NULL OR uv.voted_at < ($1::text::date + 1)) \
           AND ($2 OR (uv.vote_kind = 1 AND EXISTS (SELECT 1 FROM public.relations r \
                WHERE r.from_entity_id = uv.object_id \
                  AND r.type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid \
                  AND r.to_entity_id = '96f859ef-a1ca-4b22-9372-c86ad58b694b'::uuid))) \
         GROUP BY uv.user_id \
         ORDER BY count(*) DESC, uv.user_id",
    )
    .bind(as_of.as_deref())
    .bind(all_voters)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let options = parse_args(env::args().skip(1))?;

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let max_excluded = env::var("EXCLUSION_SYNC_MAX_EXCLUDED")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_EXCLUDED);
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;

    let known: String = sqlx::query_scalar(&format!(
        "SELECT {} FROM (SELECT $1::text AS address) s",
        SPACE_ADDRESS_HASH_SQL
    ))
    .bind(KNOWN_ADDRESS)
    .fetch_one(&pool)
    .await?;
    if known != KNOWN_HASH {
        return Err(IndexerError::Config(format!(
            "gaia's wallet hash ({known}) no longer matches analytics' ({KNOWN_HASH})"
        )));
    }

    let body = match &options.input {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| IndexerError::Config(format!("cannot read {path}: {e}")))?,
        None => fetch_from_clickhouse().await?,
    };
    let candidates = parse_rows(&body)?;
    if candidates.is_empty() {
        return Err(IndexerError::Config(
            "analytics returned no exclusions; refusing to clear the list".into(),
        ));
    }
    let unplaced = candidates
        .iter()
        .filter(|c| c.wallet_address_hash.is_none())
        .count();
    let wallets = candidates
        .iter()
        .filter_map(|c| c.wallet_address_hash.as_deref())
        .collect::<std::collections::BTreeSet<_>>()
        .len();

    let placed = place_on_spaces(&pool, &candidates).await?;
    if placed.is_empty() {
        return Err(IndexerError::Config(format!(
            "{} exclusion rows ({wallets} wallets) matched no personal space; refusing to write",
            candidates.len()
        )));
    }
    if placed.len() > max_excluded {
        return Err(IndexerError::Config(format!(
            "{} spaces would be excluded, more than EXCLUSION_SYNC_MAX_EXCLUDED={max_excluded}; refusing to write",
            placed.len()
        )));
    }

    let all_voters = voters(&pool, &options.as_of, options.all_voters).await?;
    let other_sources: Vec<uuid::Uuid> =
        sqlx::query_scalar("SELECT user_id FROM public.account_exclusions WHERE source <> $1")
            .bind(SOURCE)
            .fetch_all(&pool)
            .await?;
    let excluded_voters: Vec<&VoterRow> = all_voters
        .iter()
        .filter(|v| placed.contains_key(&v.user_id) || other_sources.contains(&v.user_id))
        .collect();
    info!(
        rows = candidates.len(),
        wallets,
        unplaced_accounts = unplaced,
        spaces_excluded = placed.len(),
        voters = all_voters.len(),
        voters_after = all_voters.len() - excluded_voters.len(),
        as_of = options.as_of.as_deref().unwrap_or("now"),
        dry_run = options.dry_run,
        "exclusions placed on personal spaces"
    );

    if options.dry_run {
        println!(
            "voters{}: {} -> {} after exclusions ({} excluded voters; {} excluded spaces in total, {} wallets without a personal space, {} excluded accounts with no wallet on record)",
            options
                .as_of
                .as_ref()
                .map(|d| format!(" as of {d}"))
                .unwrap_or_default(),
            all_voters.len(),
            all_voters.len() - excluded_voters.len(),
            excluded_voters.len(),
            placed.len(),
            wallets.saturating_sub(placed.len()),
            unplaced
        );
        println!("\nEXCLUDED voters (space id, votes, reasons, public Geo name):");
        for v in &excluded_voters {
            let reasons = placed
                .get(&v.user_id)
                .cloned()
                .unwrap_or_else(|| "non-analytics exclusion".into());
            println!(
                "  {}  {:>5}  {:<32}  {}",
                v.user_id,
                v.votes,
                reasons,
                v.name.as_deref().unwrap_or("(no name)")
            );
        }
        if options.list_kept {
            println!("\nKEPT voters:");
            for v in all_voters
                .iter()
                .filter(|v| !excluded_voters.iter().any(|x| x.user_id == v.user_id))
            {
                println!(
                    "  {}  {:>5}  {}",
                    v.user_id,
                    v.votes,
                    v.name.as_deref().unwrap_or("(no name)")
                );
            }
        }
        println!("\nDry run: nothing written.");
        return Ok(());
    }

    let rows = serde_json::Value::Array(
        placed
            .iter()
            .map(|(id, reasons)| {
                serde_json::json!({"user_id": id.to_string(), "reason": format!("analytics: {reasons}")})
            })
            .collect(),
    );
    let written: i32 =
        sqlx::query_scalar("SELECT public.replace_account_exclusions($1::jsonb, $2)")
            .bind(rows)
            .bind(SOURCE)
            .fetch_one(&pool)
            .await?;
    if written as usize != placed.len() {
        // Rows also excluded manually keep their manual reason (ON CONFLICT DO NOTHING).
        warn!(
            written,
            placed = placed.len(),
            "some spaces were already excluded by another source"
        );
    }
    info!(written, "account exclusions replaced");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rows_and_nulls() {
        let rows = parse_rows(
            "{\"wallet_address_hash\":\"sha256:ab\",\"reason\":\"plus_alias\"}\n\n{\"wallet_address_hash\":null,\"reason\":\"test_email\"}\n",
        )
        .unwrap();
        assert_eq!(
            rows,
            vec![
                Candidate {
                    wallet_address_hash: Some("sha256:ab".into()),
                    reason: "plus_alias".into()
                },
                Candidate {
                    wallet_address_hash: None,
                    reason: "test_email".into()
                },
            ]
        );
    }

    #[test]
    fn a_malformed_row_is_an_error_not_a_shorter_list() {
        assert!(
            parse_rows("{\"wallet_address_hash\":\"x\",\"reason\":\"a\"}\nnot json\n").is_err()
        );
    }

    #[test]
    fn arguments() {
        let o = parse_args(
            [
                "--dry-run",
                "--as-of",
                "2026-10-02",
                "--list-kept",
                "--input",
                "f.jsonl",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        assert!(o.dry_run && o.list_kept);
        assert_eq!(o.as_of.as_deref(), Some("2026-10-02"));
        assert_eq!(o.input.as_deref(), Some("f.jsonl"));
        assert!(parse_args(["--bogus".to_string()].into_iter()).is_err());
    }
}
