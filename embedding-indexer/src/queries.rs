//! Query and script shapes. Pure functions so their JSON is unit-tested.

use serde_json::{Value, json};

use crate::scope::{Scope, TYPES_RELATION_TYPE, source_fields};

/// Documents that should have a vector, evaluated server-side so the backfill does not page
/// through millions of nameless ids: named, live, and (if configured) of an allowed type / space.
pub fn scope_filter(scope: &Scope) -> Value {
    let mut filter = vec![json!({ "exists": { "field": "name" } })];
    if !scope.type_ids.is_empty() {
        let mut ids: Vec<&String> = scope.type_ids.iter().collect();
        ids.sort();
        filter.push(
            json!({ "nested": { "path": "relations", "query": { "bool": { "filter": [
            { "term": { "relations.relation_type": TYPES_RELATION_TYPE } },
            { "terms": { "relations.to_entity_id": ids } }
        ] } } } }),
        );
    }
    if !scope.space_ids.is_empty() {
        let mut ids: Vec<&String> = scope.space_ids.iter().collect();
        ids.sort();
        filter.push(json!({ "terms": { "space_id": ids } }));
    }
    let mut q = json!({ "bool": { "filter": filter } });
    if scope.skip_deleted {
        q["bool"]["must_not"] = json!([{ "term": { "deleted": true } }]);
    }
    q
}

/// Backfill page: in-scope documents OR documents already carrying the slot (so out-of-scope
/// leftovers get cleaned), in a stable key order that survives restarts.
pub fn backfill_query(
    scope: &Scope,
    vector_field: &str,
    after: Option<&Value>,
    size: usize,
) -> Value {
    let mut body = json!({
        "size": size,
        "_source": source_fields(vector_field),
        "sort": [{ "entity_id": "asc" }, { "space_id": "asc" }],
        "query": { "bool": { "should": [
            scope_filter(scope),
            { "exists": { "field": embedding::slots::src_hash_field(vector_field) } }
        ], "minimum_should_match": 1 } }
    });
    if let Some(a) = after {
        body["search_after"] = a.clone();
    }
    body
}

/// Follow page: everything whose `indexed_at` is at or after `since_ms`, oldest first.
pub fn follow_query(
    vector_field: &str,
    since_ms: i64,
    after: Option<&Value>,
    size: usize,
) -> Value {
    let mut body = json!({
        "size": size,
        "_source": source_fields(vector_field),
        "sort": [{ "indexed_at": "asc" }, { "entity_id": "asc" }, { "space_id": "asc" }],
        "query": { "range": { "indexed_at": { "gte": since_ms, "format": "epoch_millis" } } }
    });
    if let Some(a) = after {
        body["search_after"] = a.clone();
    }
    body
}

/// Compare-and-set: write the vector only if the document's text is still what was embedded.
/// Tombstoned documents are left alone. No `indexed_at` here: the indexer's own writes must not
/// look like content changes to its own poll.
pub const CAS_WRITE_SCRIPT: &str = r#"
    if (ctx._source.containsKey('deleted') && ctx._source.deleted == true) {
        ctx.op = 'noop';
    } else {
        def n = ctx._source.containsKey('name') ? ctx._source.name : null;
        def d = ctx._source.containsKey('description') ? ctx._source.description : null;
        if (n != params.name || d != params.description) {
            ctx.op = 'noop';
        } else {
            ctx._source[params.vec_field] = params.vector;
            ctx._source[params.hash_field] = params.src_hash;
            ctx._source[params.at_field] = params.now;
        }
    }
"#;

/// Remove the slot's fields; no-op if none are present.
pub const REMOVE_SCRIPT: &str = r#"
    boolean had = false;
    for (f in [params.vec_field, params.hash_field, params.at_field]) {
        if (ctx._source.containsKey(f)) { ctx._source.remove(f); had = true; }
    }
    if (!had) { ctx.op = 'noop'; }
"#;

pub struct SlotFields {
    pub vec: String,
    pub hash: String,
    pub at: String,
}

impl SlotFields {
    pub fn for_vector_field(vector_field: &str) -> Self {
        Self {
            vec: vector_field.to_string(),
            hash: embedding::slots::src_hash_field(vector_field),
            at: embedding::slots::at_field(vector_field),
        }
    }
}

/// Bulk `update` body for one CAS write.
pub fn cas_write_body(
    fields: &SlotFields,
    name: Option<&str>,
    description: Option<&str>,
    vector: &[f32],
    src_hash: &str,
    now: &str,
) -> Value {
    json!({ "script": { "source": CAS_WRITE_SCRIPT, "lang": "painless", "params": {
        "name": name, "description": description,
        "vec_field": fields.vec, "hash_field": fields.hash, "at_field": fields.at,
        "vector": vector, "src_hash": src_hash, "now": now
    } } })
}

pub fn remove_body(fields: &SlotFields) -> Value {
    json!({ "script": { "source": REMOVE_SCRIPT, "lang": "painless", "params": {
        "vec_field": fields.vec, "hash_field": fields.hash, "at_field": fields.at
    } } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn scope(types: &[&str], spaces: &[&str]) -> Scope {
        Scope {
            type_ids: types.iter().map(|s| s.to_string()).collect(),
            space_ids: spaces.iter().map(|s| s.to_string()).collect(),
            skip_deleted: true,
        }
    }

    #[test]
    fn scope_filter_shapes() {
        let q = scope_filter(&Scope::all_named());
        assert_eq!(
            q["bool"]["filter"][0],
            json!({ "exists": { "field": "name" } })
        );
        assert_eq!(
            q["bool"]["must_not"][0],
            json!({ "term": { "deleted": true } })
        );
        let q = scope_filter(&scope(&["t1"], &["s1"]));
        assert_eq!(q["bool"]["filter"][1]["nested"]["path"], "relations");
        assert_eq!(
            q["bool"]["filter"][1]["nested"]["query"]["bool"]["filter"][1]["terms"]["relations.to_entity_id"],
            json!(["t1"])
        );
        assert_eq!(q["bool"]["filter"][2]["terms"]["space_id"], json!(["s1"]));
        let mut s = scope(&[], &[]);
        s.skip_deleted = false;
        assert!(scope_filter(&s)["bool"].get("must_not").is_none());
    }

    #[test]
    fn backfill_and_follow_queries() {
        let b = backfill_query(&Scope::all_named(), "emb_x", None, 10);
        assert_eq!(b["size"], 10);
        assert_eq!(
            b["sort"],
            json!([{ "entity_id": "asc" }, { "space_id": "asc" }])
        );
        assert_eq!(
            b["query"]["bool"]["should"][1],
            json!({ "exists": { "field": "emb_x_src_hash" } })
        );
        assert!(b.get("search_after").is_none());
        let b = backfill_query(&Scope::all_named(), "emb_x", Some(&json!(["e", "s"])), 10);
        assert_eq!(b["search_after"], json!(["e", "s"]));
        let f = follow_query("emb_x", 1_700_000_000_000, Some(&json!([1, "e", "s"])), 5);
        assert_eq!(
            f["query"]["range"]["indexed_at"]["gte"],
            1_700_000_000_000i64
        );
        assert_eq!(f["query"]["range"]["indexed_at"]["format"], "epoch_millis");
        assert_eq!(f["sort"][0], json!({ "indexed_at": "asc" }));
        assert_eq!(f["search_after"], json!([1, "e", "s"]));
        let fields: HashSet<String> = f["_source"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(
            fields.contains("emb_x_src_hash")
                && fields.contains("name")
                && !fields.contains("emb_x")
        );
    }

    #[test]
    fn bodies_carry_the_guard_and_the_fields() {
        let f = SlotFields::for_vector_field("emb_x");
        let b = cas_write_body(
            &f,
            Some("n"),
            None,
            &[0.5, 0.5],
            "h",
            "2026-09-30T00:00:00Z",
        );
        assert_eq!(b["script"]["params"]["name"], "n");
        assert_eq!(b["script"]["params"]["description"], Value::Null);
        assert_eq!(b["script"]["params"]["vec_field"], "emb_x");
        assert_eq!(b["script"]["params"]["hash_field"], "emb_x_src_hash");
        assert_eq!(b["script"]["params"]["vector"], json!([0.5, 0.5]));
        assert!(
            CAS_WRITE_SCRIPT.contains("ctx.op = 'noop'")
                && !CAS_WRITE_SCRIPT.contains("indexed_at")
        );
        let r = remove_body(&f);
        assert_eq!(r["script"]["params"]["at_field"], "emb_x_at");
    }
}
