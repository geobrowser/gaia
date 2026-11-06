//! The ConfigMap home for `personalization.feed_signal_config` and `personalization.stance_map_config`
//! (geo-db plan 10.22 section 6): two operator-edited, one-row tables that become a mounted JSON file.
//!
//! Flag-off. Postgres stays the source: the SQL functions (`feed_signal_window`, `record_stance_map_run`) read the
//! tables, and the Rust twins read the table row unless `FEED_SIGNAL_CONFIG_SOURCE` / `STANCE_MAP_CONFIG_SOURCE` is
//! `configmap`. With the flag on, a twin takes the file instead, and the difference from the Postgres row is logged
//! by field name (never a value that could be anything but a number). A missing or invalid file falls back to the
//! Postgres row, loudly. Nothing here writes a table.
//!
//! The validators mirror the tables' CHECKs (migrations 0108, 0109) so a bad edit is refused at load, not at run time.
//! The seed values are the migrations' column defaults; the live rows may differ, which is what the diff is for.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::feed_signals_compute as compute;
use crate::stance_graph::GateConfig;

/// Where the ConfigMap is mounted unless `RANKING_CONFIG_DIR` says otherwise.
pub const DEFAULT_DIR: &str = "/etc/ranking-config";
pub const FEED_SIGNAL_FILE: &str = "feed-signal-config.json";
pub const STANCE_MAP_FILE: &str = "stance-map-config.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Postgres,
    ConfigMap,
}

impl Source {
    /// `configmap` selects the file; unset or `postgres` the table. Anything else is an error, so a typo cannot
    /// silently leave the flag off.
    pub fn parse(v: Option<&str>) -> Result<Self, String> {
        match v.map(str::trim) {
            None | Some("") | Some("postgres") => Ok(Source::Postgres),
            Some("configmap") => Ok(Source::ConfigMap),
            Some(other) => Err(format!(
                "unknown config source {other:?}; use postgres or configmap"
            )),
        }
    }

    pub fn from_env(var: &str) -> Self {
        match Self::parse(std::env::var(var).ok().as_deref()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(var, "{e}; using postgres");
                Source::Postgres
            }
        }
    }
}

pub fn dir_from_env() -> PathBuf {
    std::env::var("RANKING_CONFIG_DIR").map_or_else(|_| PathBuf::from(DEFAULT_DIR), PathBuf::from)
}

/// A row of `personalization.feed_signal_config`. Intervals are whole seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FeedSignalConfig {
    pub revise_days: i64,
    pub backfill_days: i64,
    pub user_retention_days: i64,
    pub item_retention_days: i64,
    pub run_retention_days: i64,
    pub max_run_age_secs: i64,
    pub max_data_age_secs: i64,
}

impl Default for FeedSignalConfig {
    /// Migration 0109's column defaults.
    fn default() -> Self {
        Self {
            revise_days: 7,
            backfill_days: 30,
            user_retention_days: 30,
            item_retention_days: 90,
            run_retention_days: 90,
            max_run_age_secs: 3 * 3600,
            max_data_age_secs: 6 * 3600,
        }
    }
}

impl FeedSignalConfig {
    pub fn from_json(s: &str) -> Result<Self, String> {
        let c: Self = serde_json::from_str(s).map_err(|e| format!("feed signal config: {e}"))?;
        c.validate()?;
        Ok(c)
    }

    /// The CHECKs of migration 0109.
    pub fn validate(&self) -> Result<(), String> {
        let between = |name: &str, v: i64, lo: i64, hi: i64| {
            if (lo..=hi).contains(&v) {
                Ok(())
            } else {
                Err(format!(
                    "feed signal config: {name} {v} is outside {lo}..={hi}"
                ))
            }
        };
        between("revise_days", self.revise_days, 1, 30)?;
        between("backfill_days", self.backfill_days, 1, 90)?;
        between("user_retention_days", self.user_retention_days, 1, 30)?;
        between("item_retention_days", self.item_retention_days, 7, 400)?;
        if self.run_retention_days < 1 {
            return Err(format!(
                "feed signal config: run_retention_days {} is below 1",
                self.run_retention_days
            ));
        }
        if self.max_run_age_secs < 1 || self.max_data_age_secs < 1 {
            return Err(
                "feed signal config: max_run_age_secs and max_data_age_secs must be positive"
                    .into(),
            );
        }
        if self.backfill_days < self.revise_days {
            return Err("feed signal config: backfill_days is below revise_days".into());
        }
        Ok(())
    }

    /// The window and retention knobs the Rust twin takes.
    pub fn to_compute(&self) -> compute::Config {
        compute::Config {
            revise_days: self.revise_days,
            backfill_days: self.backfill_days,
            user_retention_days: self.user_retention_days,
            item_retention_days: self.item_retention_days,
        }
    }
}

/// The four columns the twin reads from the table, in `feed_signals_sync::shadow` order.
pub type FeedSignalRow = (i32, i32, i32, i32);

/// Names of the twin's four knobs on which the file and the Postgres row differ. Empty means they agree.
pub fn feed_signal_diff(file: &FeedSignalConfig, row: FeedSignalRow) -> Vec<&'static str> {
    let (r, b, u, i) = row;
    let mut d = Vec::new();
    if file.revise_days != i64::from(r) {
        d.push("revise_days");
    }
    if file.backfill_days != i64::from(b) {
        d.push("backfill_days");
    }
    if file.user_retention_days != i64::from(u) {
        d.push("user_retention_days");
    }
    if file.item_retention_days != i64::from(i) {
        d.push("item_retention_days");
    }
    d
}

/// A row of `personalization.stance_map_config` as a file.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct StanceMapConfig {
    pub min_clean_votes: i64,
    pub min_split_correlation: f64,
    pub min_auc_lift: f64,
    pub stated_weight: f64,
}

impl Default for StanceMapConfig {
    fn default() -> Self {
        let g = GateConfig::default();
        Self {
            min_clean_votes: g.min_clean_votes,
            min_split_correlation: g.min_split_correlation,
            min_auc_lift: g.min_auc_lift,
            stated_weight: g.stated_weight,
        }
    }
}

impl StanceMapConfig {
    pub fn from_json(s: &str) -> Result<Self, String> {
        let c: Self = serde_json::from_str(s).map_err(|e| format!("stance map config: {e}"))?;
        c.validate()?;
        Ok(c)
    }

    /// The CHECK of migration 0108 (`stance_map_config_ranges`); `min_auc_lift` has none but must be finite.
    pub fn validate(&self) -> Result<(), String> {
        if self.min_clean_votes < 0 {
            return Err(format!(
                "stance map config: min_clean_votes {} is negative",
                self.min_clean_votes
            ));
        }
        if !self.min_split_correlation.is_finite()
            || !(0.0..=1.0).contains(&self.min_split_correlation)
        {
            return Err("stance map config: min_split_correlation is outside 0..=1".into());
        }
        if !self.min_auc_lift.is_finite() {
            return Err("stance map config: min_auc_lift is not finite".into());
        }
        if !self.stated_weight.is_finite() || self.stated_weight < 0.0 {
            return Err("stance map config: stated_weight is negative or not finite".into());
        }
        Ok(())
    }

    pub fn to_gate(&self) -> GateConfig {
        GateConfig {
            min_clean_votes: self.min_clean_votes,
            min_split_correlation: self.min_split_correlation,
            min_auc_lift: self.min_auc_lift,
            stated_weight: self.stated_weight,
        }
    }
}

/// Names of the fields on which the file and the Postgres row differ. Empty means they agree.
pub fn stance_map_diff(file: &StanceMapConfig, row: &GateConfig) -> Vec<&'static str> {
    let mut d = Vec::new();
    if file.min_clean_votes != row.min_clean_votes {
        d.push("min_clean_votes");
    }
    if file.min_split_correlation != row.min_split_correlation {
        d.push("min_split_correlation");
    }
    if file.min_auc_lift != row.min_auc_lift {
        d.push("min_auc_lift");
    }
    if file.stated_weight != row.stated_weight {
        d.push("stated_weight");
    }
    d
}

fn read(dir: &Path, name: &str) -> Result<String, String> {
    std::fs::read_to_string(dir.join(name))
        .map_err(|e| format!("{}: {e}", dir.join(name).display()))
}

pub fn load_feed_signal(dir: &Path) -> Result<FeedSignalConfig, String> {
    FeedSignalConfig::from_json(&read(dir, FEED_SIGNAL_FILE)?)
}

pub fn load_stance_map(dir: &Path) -> Result<StanceMapConfig, String> {
    StanceMapConfig::from_json(&read(dir, STANCE_MAP_FILE)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_defaults_to_postgres_and_refuses_typos() {
        assert_eq!(Source::parse(None), Ok(Source::Postgres));
        assert_eq!(Source::parse(Some("")), Ok(Source::Postgres));
        assert_eq!(Source::parse(Some("postgres")), Ok(Source::Postgres));
        assert_eq!(Source::parse(Some("configmap")), Ok(Source::ConfigMap));
        assert!(Source::parse(Some("config-map")).is_err());
    }

    #[test]
    fn feed_signal_defaults_are_the_migration_defaults_and_valid() {
        let d = FeedSignalConfig::default();
        assert_eq!(d.validate(), Ok(()));
        assert_eq!(FeedSignalConfig::from_json("{}"), Ok(d));
        assert_eq!(d.to_compute().revise_days, 7);
        assert_eq!((d.max_run_age_secs, d.max_data_age_secs), (10_800, 21_600));
    }

    #[test]
    fn feed_signal_validation_mirrors_the_checks() {
        for bad in [
            r#"{"revise_days":0}"#,
            r#"{"revise_days":31}"#,
            r#"{"backfill_days":91}"#,
            r#"{"user_retention_days":31}"#,
            r#"{"item_retention_days":6}"#,
            r#"{"item_retention_days":401}"#,
            r#"{"run_retention_days":0}"#,
            r#"{"max_run_age_secs":0}"#,
            r#"{"revise_days":10,"backfill_days":9}"#,
            r#"{"revise_days":"7"}"#,
            r#"{"unknown":1}"#,
            "not json",
        ] {
            assert!(FeedSignalConfig::from_json(bad).is_err(), "{bad}");
        }
        assert!(FeedSignalConfig::from_json(r#"{"revise_days":30,"backfill_days":30}"#).is_ok());
    }

    #[test]
    fn stance_map_defaults_match_gate_config_and_validation_mirrors_the_check() {
        let d = StanceMapConfig::default();
        assert_eq!(d.to_gate(), GateConfig::default());
        assert_eq!(StanceMapConfig::from_json("{}"), Ok(d));
        for bad in [
            r#"{"min_clean_votes":-1}"#,
            r#"{"min_split_correlation":1.01}"#,
            r#"{"min_split_correlation":-0.1}"#,
            r#"{"stated_weight":-1}"#,
            r#"{"extra":1}"#,
        ] {
            assert!(StanceMapConfig::from_json(bad).is_err(), "{bad}");
        }
        assert!(
            StanceMapConfig::from_json(r#"{"min_split_correlation":1,"stated_weight":0}"#).is_ok()
        );
    }

    #[test]
    fn diffs_name_the_fields_that_differ() {
        let f = FeedSignalConfig::default();
        assert!(feed_signal_diff(&f, (7, 30, 30, 90)).is_empty());
        assert_eq!(
            feed_signal_diff(&f, (5, 30, 30, 120)),
            vec!["revise_days", "item_retention_days"]
        );
        let s = StanceMapConfig::default();
        assert!(stance_map_diff(&s, &GateConfig::default()).is_empty());
        let g = GateConfig {
            min_clean_votes: 5,
            ..GateConfig::default()
        };
        assert_eq!(stance_map_diff(&s, &g), vec!["min_clean_votes"]);
    }

    #[test]
    fn loads_from_a_directory() {
        let d = std::env::temp_dir().join(format!("ranking-config-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(FEED_SIGNAL_FILE), r#"{"revise_days":3}"#).unwrap();
        std::fs::write(d.join(STANCE_MAP_FILE), "{}").unwrap();
        assert_eq!(load_feed_signal(&d).unwrap().revise_days, 3);
        assert_eq!(load_stance_map(&d).unwrap(), StanceMapConfig::default());
        assert!(load_feed_signal(&d.join("missing")).is_err());
        std::fs::remove_dir_all(&d).ok();
    }
}
