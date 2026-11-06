//! File-backed `app_webhooks` filters, mounted from a Kubernetes Secret.
//!
//! Enabled by `APP_WEBHOOKS_FILE=<path>`: a JSON array of
//! `{id, app_name, url, secret, notification_types[], space_ids[]}`. The
//! indexer only needs the filter columns, so `url` and `secret` are never
//! deserialized (unknown keys are ignored) and cannot be logged from here.
//! When the variable is unset the indexer keeps reading Postgres.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use uuid::Uuid;

use crate::models::KNOWN_NOTIFICATION_TYPES;
use crate::storage::WebhookFilter;

/// Env var naming the mounted JSON file. Unset or empty = Postgres path.
pub const ENV_VAR: &str = "APP_WEBHOOKS_FILE";

#[derive(Debug, Clone, Deserialize)]
pub struct FileWebhook {
    pub id: Uuid,
    #[serde(default)]
    pub app_name: String,
    #[serde(default)]
    pub notification_types: Option<Vec<String>>,
    #[serde(default)]
    pub space_ids: Option<Vec<Uuid>>,
}

impl FileWebhook {
    pub fn filter(&self) -> WebhookFilter {
        WebhookFilter {
            id: self.id,
            notification_types: self.notification_types.clone(),
            space_ids: self.space_ids.clone(),
        }
    }
}

/// Parse the JSON array. The error text never contains file contents.
pub fn parse(bytes: &[u8]) -> Result<Vec<FileWebhook>, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("invalid app webhooks JSON: {e}"))
}

/// Read and parse the file at `path`.
pub async fn read(path: &Path) -> Result<Vec<FileWebhook>, String> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| format!("cannot read app webhooks file: {e}"))?;
    parse(&bytes)
}

/// `Some(path)` when `APP_WEBHOOKS_FILE` is set to a non-empty value.
pub fn path_from_env() -> Option<PathBuf> {
    let p = std::env::var(ENV_VAR).ok()?;
    let p = p.trim();
    (!p.is_empty()).then(|| PathBuf::from(p))
}

/// `(app_name, token)` for every filter token the indexer never emits.
pub fn unknown_filter_types(hooks: &[FileWebhook]) -> Vec<(String, String)> {
    hooks
        .iter()
        .flat_map(|h| {
            h.notification_types
                .iter()
                .flatten()
                .filter(|t| !KNOWN_NOTIFICATION_TYPES.contains(&t.as_str()))
                .map(|t| (h.app_name.clone(), t.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "00000000-0000-0000-0000-00000000000a";
    const B: &str = "00000000-0000-0000-0000-00000000000b";

    fn sample() -> String {
        format!(
            r#"[
              {{"id":"{A}","app_name":"one","url":"https://one.example","secret":"s3cr3t",
                "notification_types":["proposal_created","bogus_type"],"space_ids":["{B}"]}},
              {{"id":"{B}","app_name":"two","url":"https://two.example","secret":"s3cr3t",
                "notification_types":null,"space_ids":null}}
            ]"#
        )
    }

    #[test]
    fn parses_filters() {
        let hooks = parse(sample().as_bytes()).unwrap();
        assert_eq!(hooks.len(), 2);
        let f = hooks[0].filter();
        assert_eq!(f.id, A.parse::<Uuid>().unwrap());
        assert_eq!(
            f.space_ids.as_deref(),
            Some(&[B.parse::<Uuid>().unwrap()][..])
        );
        assert!(hooks[1].filter().notification_types.is_none());
    }

    #[test]
    fn filter_lookup_matches_like_postgres_rows() {
        let hooks = parse(sample().as_bytes()).unwrap();
        let ev = ["proposal_created".to_string()];
        let space_b: Uuid = B.parse().unwrap();
        let space_x = Uuid::from_u128(0xdead);
        assert!(hooks[0].filter().matches(&ev, space_b));
        assert!(!hooks[0].filter().matches(&ev, space_x));
        assert!(hooks[1].filter().matches(&ev, space_x));
    }

    #[test]
    fn flags_unknown_types_only() {
        let hooks = parse(sample().as_bytes()).unwrap();
        assert_eq!(
            unknown_filter_types(&hooks),
            vec![("one".to_string(), "bogus_type".to_string())]
        );
    }

    #[test]
    fn parse_error_does_not_echo_values() {
        let err = parse(br#"[{"id":"nope","secret":"hunter2"}]"#).unwrap_err();
        assert!(!err.contains("hunter2"), "{err}");
    }

    #[test]
    fn unset_env_means_postgres_path() {
        std::env::remove_var(ENV_VAR);
        assert!(path_from_env().is_none());
    }
}
