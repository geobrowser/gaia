//! `personalization.wallet_address_hash` (migration 0109) as a Rust function, and a shadow that checks the two agree.
//!
//! The SQL function is a pure hash: `'sha256:' || hex(sha256('wallet:' || lower(btrim(address))))`, analytics'
//! `identity_hash('wallet', address)`. Its home once Postgres is gone is code, not a table: a job that joins
//! analytics' hashes to a Space's address hashes the address itself, over the address geo-db holds. This is that
//! function, with the same trim and case rules, so a join over geo-db's addresses needs no Postgres call.
//!
//! The shadow (`WALLET_HASH_RUST_SHADOW`, off by default) reads a bounded sample of Personal Space addresses with
//! the SQL hash beside them, hashes each in Rust, and logs how many agree. It only reads, never fails the job, and
//! logs counts, never an address or a hash.

use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tracing::{info, warn};

/// How many Personal Space addresses one shadow pass compares.
pub const SHADOW_SAMPLE: i64 = 500;

/// `lower(btrim(address))` as Postgres computes it for the addresses a Space holds. `btrim` with no
/// second argument removes spaces only, so this trims `' '` and not other whitespace.
fn normalise(address: &str) -> String {
    address.trim_matches(' ').to_lowercase()
}

/// analytics' `identity_hash("wallet", address)`: `sha256:` and the lowercase hex of
/// `sha256("wallet:" + lower(trim(address)))`. Same bytes as the SQL function.
pub fn wallet_address_hash(address: &str) -> String {
    let digest = Sha256::digest(format!("wallet:{}", normalise(address)).as_bytes());
    let mut out = String::with_capacity(7 + 64);
    out.push_str("sha256:");
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// What one shadow pass saw.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub checked: usize,
    pub matched: usize,
    pub mismatched: usize,
}

/// Compares `(address, hash Postgres computed)` pairs with the Rust hash. Pure.
pub fn compare(pairs: &[(String, String)]) -> Report {
    let mut report = Report::default();
    for (address, sql_hash) in pairs {
        report.checked += 1;
        if wallet_address_hash(address) == *sql_hash {
            report.matched += 1;
        } else {
            report.mismatched += 1;
        }
    }
    report
}

/// Whether `WALLET_HASH_RUST_SHADOW` is `1` or `true` in `get`'s environment.
pub fn shadow_enabled_from(get: impl Fn(&str) -> Option<String>) -> bool {
    matches!(
        get("WALLET_HASH_RUST_SHADOW").as_deref().map(str::trim),
        Some("1") | Some("true")
    )
}

/// Whether the shadow is on in the process environment.
pub fn shadow_enabled() -> bool {
    shadow_enabled_from(|k| std::env::var(k).ok())
}

/// One shadow pass: reads, compares, logs counts. A read error is logged and ends the pass.
pub async fn shadow(pool: &PgPool) -> Option<Report> {
    let rows: Result<Vec<(String, String)>, sqlx::Error> = sqlx::query_as(
        "SELECT s.address, personalization.wallet_address_hash(s.address) \
         FROM public.spaces s WHERE s.type = 'Personal' AND s.address IS NOT NULL \
         ORDER BY s.id LIMIT $1",
    )
    .bind(SHADOW_SAMPLE)
    .fetch_all(pool)
    .await;
    match rows {
        Ok(pairs) => {
            let report = compare(&pairs);
            if report.mismatched == 0 {
                info!(
                    checked = report.checked,
                    matched = report.matched,
                    "wallet hash shadow: Rust and SQL agree"
                );
            } else {
                warn!(
                    checked = report.checked,
                    matched = report.matched,
                    mismatched = report.mismatched,
                    "wallet hash shadow: Rust and SQL differ"
                );
            }
            Some(report)
        }
        Err(e) => {
            warn!(error = %e, "wallet hash shadow: read failed");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clickhouse::{KNOWN_ADDRESS, KNOWN_HASH};

    #[test]
    fn matches_the_pair_analytics_and_sql_are_checked_against() {
        assert_eq!(wallet_address_hash(KNOWN_ADDRESS), KNOWN_HASH);
    }

    #[test]
    fn ignores_case_and_surrounding_spaces_only() {
        assert_eq!(
            wallet_address_hash("  0x929E5195F039E0BECB79B039339077FA17183064 "),
            KNOWN_HASH
        );
        assert_ne!(
            wallet_address_hash("\t0x929e5195f039e0becb79b039339077fa17183064"),
            KNOWN_HASH
        );
    }

    #[test]
    fn compare_counts_each_side() {
        let pairs = vec![
            (KNOWN_ADDRESS.to_string(), KNOWN_HASH.to_string()),
            (KNOWN_ADDRESS.to_string(), "sha256:00".to_string()),
        ];
        assert_eq!(
            compare(&pairs),
            Report {
                checked: 2,
                matched: 1,
                mismatched: 1
            }
        );
        assert_eq!(compare(&[]), Report::default());
    }

    #[test]
    fn flag_is_off_unless_one_or_true() {
        assert!(!shadow_enabled_from(|_| None));
        assert!(!shadow_enabled_from(|_| Some("0".into())));
        assert!(shadow_enabled_from(|_| Some(" 1 ".into())));
        assert!(shadow_enabled_from(|_| Some("true".into())));
    }
}
