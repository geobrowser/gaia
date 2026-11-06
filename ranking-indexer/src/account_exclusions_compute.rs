//! Rust twin of `public.replace_account_exclusions(rows, source_name)` (migration 0101): the new
//! `account_exclusions` table and the written count, given the table before. The SQL stays the
//! source; `account_exclusions_sync` runs this as a shadow behind ACCOUNT_EXCLUSIONS_RUST_COMPUTE
//! and only compares.
//!
//! Rows already excluded by another source keep their reason (`ON CONFLICT DO NOTHING`), so the
//! written count can be below the list's length. The SQL picks an arbitrary row among duplicates
//! of one user (`DISTINCT ON` with no `ORDER BY`); this takes the first, and analytics sends one
//! row per user.

use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub user_id: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Exclusion {
    pub user_id: String,
    pub reason: String,
    pub source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The list was null, not an array, or empty.
    EmptyList,
    /// A row to insert has no reason (the column is NOT NULL).
    NullReason,
}

/// `rows` is `None` for a null or non-array argument. Returns the written count and the table.
pub fn replace(
    existing: &[Exclusion],
    rows: Option<&[Row]>,
    source: &str,
) -> Result<(i64, BTreeSet<Exclusion>), Refusal> {
    let rows = match rows {
        Some(r) if !r.is_empty() => r,
        _ => return Err(Refusal::EmptyList),
    };
    let mut table: BTreeSet<Exclusion> = existing
        .iter()
        .filter(|e| e.source != source)
        .cloned()
        .collect();
    let mut seen = BTreeSet::new();
    let mut written = 0;
    for r in rows {
        if !seen.insert(r.user_id.clone()) {
            continue;
        }
        let reason = r.reason.clone().ok_or(Refusal::NullReason)?;
        if table.iter().any(|e| e.user_id == r.user_id) {
            continue;
        }
        table.insert(Exclusion {
            user_id: r.user_id.clone(),
            reason,
            source: source.to_string(),
        });
        written += 1;
    }
    Ok((written, table))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN: &str = include_str!("../tests/fixtures/account_exclusions_golden.txt");

    #[derive(Default)]
    struct Case {
        source: String,
        array: bool,
        existing: Vec<Exclusion>,
        rows: Vec<Row>,
        expect: Option<Result<i64, String>>,
        table: BTreeSet<Exclusion>,
    }

    #[test]
    fn replace_matches_sql() {
        let mut cases: std::collections::BTreeMap<i64, Case> = Default::default();
        for l in GOLDEN.lines() {
            let f: Vec<&str> = l.split('|').collect();
            let c = cases.entry(f[1].parse().unwrap()).or_default();
            match f[0] {
                "C" => {
                    c.source = f[2].into();
                    c.array = f[3] == "array";
                }
                "E" => c.existing.push(Exclusion {
                    user_id: f[2].into(),
                    reason: f[3].into(),
                    source: f[4].into(),
                }),
                "I" => c.rows.push(Row {
                    user_id: f[2].into(),
                    reason: (!f[3].is_empty()).then(|| f[3].to_string()),
                }),
                "Z" if f[2] == "ok" => c.expect = Some(Ok(f[3].parse().unwrap())),
                "Z" => c.expect = Some(Err(f[3].into())),
                "Q" => {
                    c.table.insert(Exclusion {
                        user_id: f[2].into(),
                        reason: f[3].into(),
                        source: f[4].into(),
                    });
                }
                t => panic!("tag {t}"),
            }
        }
        assert_eq!(cases.len(), 300);
        let (mut oks, mut refused, mut nulls) = (0, 0, 0);
        for (n, c) in &cases {
            let got = replace(&c.existing, c.array.then_some(&c.rows[..]), &c.source);
            match (c.expect.clone().unwrap(), got) {
                (Ok(w), Ok((written, table))) => {
                    oks += 1;
                    assert_eq!(written, w, "case {n} written");
                    assert_eq!(table, c.table, "case {n} table");
                }
                (Err(e), Err(r)) => {
                    match r {
                        Refusal::EmptyList => {
                            refused += 1;
                            assert_eq!(e, "refused", "case {n}");
                        }
                        Refusal::NullReason => {
                            nulls += 1;
                            assert_eq!(e, "null", "case {n}");
                        }
                    };
                }
                (want, got) => panic!("case {n}: SQL {want:?}, Rust {got:?}"),
            }
        }
        assert!(
            oks > 150 && refused > 50 && nulls > 10,
            "{oks} {refused} {nulls}"
        );
    }
}
