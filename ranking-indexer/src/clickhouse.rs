//! The analytics ClickHouse over HTTPS: read by the jobs that bring analytics data into gaia
//! (`account_exclusions_sync`, `feed_signals_sync`), each as a ClickHouse user granted SELECT on the
//! views analytics publishes for it alone; written by the one that sends gaia data to analytics
//! (`agenda_listings_refresh`, GEO-3556), as a user granted INSERT on its own table alone.
//!
//! Credentials come from CLICKHOUSE_URL, CLICKHOUSE_USER and CLICKHOUSE_PASSWORD, mounted from the
//! job's own Secret.

use std::env;
use std::time::Duration;

use crate::error::IndexerError;

/// identity_hash('wallet', KNOWN_ADDRESS) as analytics computes it
/// (crates/analytics-ingest/src/identity.rs): 'sha256:' || hex(sha256('wallet:' || lower(address))).
/// Jobs that match analytics' wallet hashes to `spaces.address` check this pair against Postgres at
/// startup, so a change to either side's hashing fails loudly instead of silently matching nobody.
pub const KNOWN_ADDRESS: &str = "0x929e5195f039E0becB79B039339077FA17183064";
pub const KNOWN_HASH: &str =
    "sha256:645c48b3494f9f1fba91b84d2c1fa55d23cfa134be3e1e488803286c13bff73b";

/// Runs `sql` and returns the response body. A non-2xx response is an error carrying the start of
/// ClickHouse's message, never an empty result.
pub async fn query(sql: &str, timeout: Duration) -> Result<String, IndexerError> {
    send(&[], sql.to_string(), timeout).await
}

/// True when this job has ClickHouse credentials at all.
pub fn configured() -> bool {
    env::var("CLICKHOUSE_URL").is_ok_and(|url| !url.trim().is_empty())
}

/// Inserts `rows`, one JSON object each, into `table` (`database.table`). Strict: a field the table
/// has no column for is refused, so a row that outgrew its warehouse migration fails the run instead
/// of losing the field. Timestamps may come as Postgres renders them (ISO 8601 with an offset).
pub async fn insert_json_each_row(
    table: &str,
    rows: &[String],
    timeout: Duration,
) -> Result<(), IndexerError> {
    let query = format!("INSERT INTO {table} FORMAT JSONEachRow");
    for batch in rows.chunks(INSERT_BATCH_ROWS) {
        send(
            &[
                ("query", query.as_str()),
                ("date_time_input_format", "best_effort"),
                ("input_format_skip_unknown_fields", "0"),
            ],
            batch.join("\n"),
            timeout,
        )
        .await?;
    }
    Ok(())
}

/// Rows per INSERT. The tables written are small; this only bounds one request.
const INSERT_BATCH_ROWS: usize = 20_000;

async fn send(
    params: &[(&str, &str)],
    body: String,
    timeout: Duration,
) -> Result<String, IndexerError> {
    let url = env::var("CLICKHOUSE_URL")
        .map_err(|_| IndexerError::Config("CLICKHOUSE_URL not set (or pass --input)".into()))?;
    let user = env::var("CLICKHOUSE_USER")
        .map_err(|_| IndexerError::Config("CLICKHOUSE_USER not set".into()))?;
    let password = env::var("CLICKHOUSE_PASSWORD")
        .map_err(|_| IndexerError::Config("CLICKHOUSE_PASSWORD not set".into()))?;
    let response = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| IndexerError::Config(format!("http client: {e}")))?
        .post(url.trim_end_matches('/').to_string() + "/")
        .query(params)
        .header("X-ClickHouse-User", user)
        .header("X-ClickHouse-Key", password)
        .body(body)
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

/// Parses JSONEachRow output. Blank lines are skipped; a malformed line is an error, never a
/// silently shorter result.
pub fn parse_json_each_row<T: serde::de::DeserializeOwned>(
    body: &str,
    what: &str,
) -> Result<Vec<T>, IndexerError> {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_str::<T>(line)
                .map_err(|e| IndexerError::Config(format!("unreadable {what} row {line:?}: {e}")))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, serde::Deserialize, PartialEq)]
    struct Row {
        a: u32,
    }

    #[test]
    fn parses_rows_and_refuses_a_malformed_line() {
        assert_eq!(
            parse_json_each_row::<Row>("{\"a\":1}\n\n{\"a\":2}\n", "test").unwrap(),
            vec![Row { a: 1 }, Row { a: 2 }]
        );
        assert!(parse_json_each_row::<Row>("{\"a\":1}\nnope\n", "test").is_err());
        assert!(parse_json_each_row::<Row>("{\"a\":\"1\"}\n", "test").is_err());
    }
}
