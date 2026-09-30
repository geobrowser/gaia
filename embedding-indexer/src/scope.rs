//! Which documents get a vector, and what to do with each one the poll returns.

use std::collections::HashSet;

use embedding::template;
use serde_json::Value;

use crate::config::Config;
use crate::error::{IndexerError, Result};

/// The Types relation type, dashed, as the search document stores relation types.
pub const TYPES_RELATION_TYPE: &str = sdk::core::ids::TYPE_RELATION_TYPE_ID;

/// Normalize an id to the dashed lowercase UUID form the search documents use.
pub fn dashed(id: &str) -> Result<String> {
    uuid::Uuid::parse_str(id)
        .map(|u| u.to_string())
        .map_err(|_| IndexerError::fatal(format!("not a UUID: {id:?}")))
}

#[derive(Debug, Clone)]
pub struct Scope {
    pub type_ids: HashSet<String>,
    pub space_ids: HashSet<String>,
    pub skip_deleted: bool,
}

impl Scope {
    pub fn from_config(cfg: &Config) -> Result<Self> {
        let norm = |ids: &[String]| -> Result<HashSet<String>> {
            ids.iter()
                .filter(|s| !s.is_empty())
                .map(|s| dashed(s))
                .collect()
        };
        Ok(Self {
            type_ids: norm(&cfg.scope_type_ids)?,
            space_ids: norm(&cfg.scope_space_ids)?,
            skip_deleted: cfg.skip_deleted,
        })
    }

    pub fn all_named() -> Self {
        Self {
            type_ids: HashSet::new(),
            space_ids: HashSet::new(),
            skip_deleted: true,
        }
    }
}

/// The parts of a search document the indexer looks at.
#[derive(Debug, Clone, PartialEq)]
pub struct Doc {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub deleted: bool,
    pub space_id: Option<String>,
    pub type_ids: Vec<String>,
    /// `emb_<slot>_src_hash`, if the document carries the slot.
    pub src_hash: Option<String>,
    /// `emb_<slot>_at` present: the slot fields exist even if the hash is missing.
    pub has_slot_fields: bool,
    /// Sort values from the hit, for `search_after`.
    pub sort: Option<Value>,
}

/// Source fields the poll requests. The vector itself is never read back.
pub fn source_fields(vector_field: &str) -> Vec<String> {
    vec![
        "name".into(),
        "description".into(),
        "deleted".into(),
        "space_id".into(),
        "relations".into(),
        embedding::slots::src_hash_field(vector_field),
        embedding::slots::at_field(vector_field),
    ]
}

pub fn parse_hit(hit: &Value, vector_field: &str) -> Doc {
    let src = &hit["_source"];
    let str_field = |k: &str| src.get(k).and_then(Value::as_str).map(str::to_string);
    let type_ids = src["relations"]
        .as_array()
        .map(|rels| {
            rels.iter()
                .filter(|r| {
                    r["relation_type"]
                        .as_str()
                        .is_some_and(|t| t.eq_ignore_ascii_case(TYPES_RELATION_TYPE))
                })
                .filter_map(|r| r["to_entity_id"].as_str().map(|s| s.to_lowercase()))
                .collect()
        })
        .unwrap_or_default();
    let src_hash = str_field(&embedding::slots::src_hash_field(vector_field));
    let has_at = src.get(embedding::slots::at_field(vector_field)).is_some();
    Doc {
        id: hit["_id"].as_str().unwrap_or_default().to_string(),
        name: str_field("name"),
        description: str_field("description"),
        deleted: src["deleted"].as_bool().unwrap_or(false),
        space_id: str_field("space_id").map(|s| s.to_lowercase()),
        type_ids,
        has_slot_fields: src_hash.is_some() || has_at,
        src_hash,
        sort: hit.get("sort").cloned(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Nothing to do and nothing to clean (out of scope, no slot fields; or tombstoned).
    Skip(&'static str),
    /// Out of scope but still carrying slot fields: remove them.
    Remove,
    /// In scope and the stored hash matches the current text.
    Unchanged,
    /// In scope and the text is new or changed.
    Embed { text: String, hash: String },
}

pub fn in_scope(doc: &Doc, scope: &Scope) -> bool {
    if !scope.type_ids.is_empty() && !doc.type_ids.iter().any(|t| scope.type_ids.contains(t)) {
        return false;
    }
    if !scope.space_ids.is_empty()
        && !doc
            .space_id
            .as_ref()
            .is_some_and(|s| scope.space_ids.contains(s))
    {
        return false;
    }
    true
}

pub fn classify(doc: &Doc, scope: &Scope, text_template: &str) -> Result<Action> {
    if doc.deleted && scope.skip_deleted {
        return Ok(Action::Skip("deleted"));
    }
    let text = template::apply(
        text_template,
        doc.name.as_deref(),
        doc.description.as_deref(),
    )
    .map_err(IndexerError::fatal)?;
    let Some(text) = text else {
        return Ok(if doc.has_slot_fields {
            Action::Remove
        } else {
            Action::Skip("nameless")
        });
    };
    if !in_scope(doc, scope) {
        return Ok(if doc.has_slot_fields {
            Action::Remove
        } else {
            Action::Skip("out of scope")
        });
    }
    let hash = template::content_hash(&text);
    if doc.src_hash.as_deref() == Some(hash.as_str()) {
        return Ok(Action::Unchanged);
    }
    Ok(Action::Embed { text, hash })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const T1: &str = "00000000-0000-0000-0000-000000000b01";
    const T2: &str = "00000000-0000-0000-0000-000000000b02";

    fn hit(
        name: Option<&str>,
        desc: Option<&str>,
        deleted: bool,
        types: &[&str],
        src_hash: Option<&str>,
    ) -> Value {
        let rels: Vec<Value> = types
            .iter()
            .map(|t| json!({"relation_id": "r", "relation_type": TYPES_RELATION_TYPE, "to_entity_id": t}))
            .collect();
        let mut src = json!({"space_id": "00000000-0000-4000-8000-000000000001", "relations": rels, "deleted": deleted});
        if let Some(n) = name {
            src["name"] = json!(n);
        }
        if let Some(d) = desc {
            src["description"] = json!(d);
        }
        if let Some(h) = src_hash {
            src["emb_s1_src_hash"] = json!(h);
            src["emb_s1_at"] = json!("2026-09-30T00:00:00Z");
        }
        json!({"_id": "e_s", "_source": src, "sort": ["a", "b"]})
    }

    fn scope(types: &[&str]) -> Scope {
        Scope {
            type_ids: types.iter().map(|s| s.to_string()).collect(),
            space_ids: HashSet::new(),
            skip_deleted: true,
        }
    }

    #[test]
    fn parse_reads_types_from_relations_and_slot_fields() {
        let d = parse_hit(
            &hit(Some("N"), Some("D"), false, &[T1, T2], Some("h")),
            "emb_s1",
        );
        assert_eq!(d.name.as_deref(), Some("N"));
        assert_eq!(d.type_ids, vec![T1.to_string(), T2.to_string()]);
        assert_eq!(d.src_hash.as_deref(), Some("h"));
        assert!(d.has_slot_fields);
        assert_eq!(d.sort, Some(json!(["a", "b"])));
        let d = parse_hit(&hit(Some("N"), None, false, &[], None), "emb_s1");
        assert!(!d.has_slot_fields && d.type_ids.is_empty());
    }

    #[test]
    fn classify_embeds_in_scope_named_docs_and_detects_unchanged() {
        let d = parse_hit(&hit(Some("Bitcoin"), None, false, &[T1], None), "emb_s1");
        let Action::Embed { text, hash } =
            classify(&d, &scope(&[T1]), template::NAME_DESCRIPTION_V1).unwrap()
        else {
            panic!()
        };
        assert_eq!(text, "Bitcoin");
        let same = parse_hit(
            &hit(Some("Bitcoin"), None, false, &[T1], Some(&hash)),
            "emb_s1",
        );
        assert_eq!(
            classify(&same, &scope(&[T1]), template::NAME_DESCRIPTION_V1).unwrap(),
            Action::Unchanged
        );
        let changed = parse_hit(
            &hit(Some("Bitcoin"), Some("a coin"), false, &[T1], Some(&hash)),
            "emb_s1",
        );
        assert!(matches!(
            classify(&changed, &scope(&[T1]), template::NAME_DESCRIPTION_V1).unwrap(),
            Action::Embed { .. }
        ));
    }

    #[test]
    fn classify_skips_or_removes_out_of_scope() {
        let tpl = template::NAME_DESCRIPTION_V1;
        // nameless: skip, or remove if it still has fields
        assert_eq!(
            classify(
                &parse_hit(&hit(None, Some("d"), false, &[T1], None), "emb_s1"),
                &scope(&[T1]),
                tpl
            )
            .unwrap(),
            Action::Skip("nameless")
        );
        assert_eq!(
            classify(
                &parse_hit(&hit(None, None, false, &[T1], Some("h")), "emb_s1"),
                &scope(&[T1]),
                tpl
            )
            .unwrap(),
            Action::Remove
        );
        // wrong type under an allowlist
        assert_eq!(
            classify(
                &parse_hit(&hit(Some("N"), None, false, &[T2], None), "emb_s1"),
                &scope(&[T1]),
                tpl
            )
            .unwrap(),
            Action::Skip("out of scope")
        );
        assert_eq!(
            classify(
                &parse_hit(&hit(Some("N"), None, false, &[T2], Some("h")), "emb_s1"),
                &scope(&[T1]),
                tpl
            )
            .unwrap(),
            Action::Remove
        );
        // no allowlist: every named doc is in scope
        assert!(matches!(
            classify(
                &parse_hit(&hit(Some("N"), None, false, &[], None), "emb_s1"),
                &Scope::all_named(),
                tpl
            )
            .unwrap(),
            Action::Embed { .. }
        ));
        // tombstoned: left alone even with fields
        assert_eq!(
            classify(
                &parse_hit(&hit(Some("N"), None, true, &[T1], Some("h")), "emb_s1"),
                &scope(&[T1]),
                tpl
            )
            .unwrap(),
            Action::Skip("deleted")
        );
    }

    #[test]
    fn dashed_accepts_both_forms() {
        assert_eq!(dashed("00000000000000000000000000000B01").unwrap(), T1);
        assert_eq!(dashed(T1).unwrap(), T1);
        assert!(dashed("nope").is_err());
    }
}
