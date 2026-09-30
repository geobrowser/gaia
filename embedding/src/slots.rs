//! What a slot occupies in the OpenSearch entities index, as pure data: the three per-slot field
//! mappings, the `_meta` bookkeeping (`embedding_slots`, `embedding_default_slot`), and the hybrid
//! search pipeline. No I/O here so every rule is unit-tested; `search-admin` does the calls and
//! `embedding-indexer` reads the same field names.

use serde_json::{Map, Value, json};

use crate::descriptor::Descriptor;
use crate::error::{Error, Result};

pub const META_SLOTS: &str = "embedding_slots";
pub const META_DEFAULT: &str = "embedding_default_slot";

/// HNSW graph parameters for every slot (measured in the step 0 PoC; `ef_search` is per query).
pub const HNSW_M: u32 = 16;
pub const HNSW_EF_CONSTRUCTION: u32 = 128;

/// The field holding the SHA-256 of the text that was embedded into `emb_<slot>`.
pub fn src_hash_field(vector_field: &str) -> String {
    format!("{vector_field}_src_hash")
}

/// The field holding when `emb_<slot>` was last written.
pub fn at_field(vector_field: &str) -> String {
    format!("{vector_field}_at")
}

/// The three fields a slot occupies: the vector, the hash of the embedded text, and the write time.
pub fn slot_field_mappings(d: &Descriptor) -> Map<String, Value> {
    let field = d.vector_field();
    let mut m = Map::new();
    m.insert(
        field.clone(),
        json!({
            "type": "knn_vector",
            "dimension": d.dimensions,
            "method": {
                "name": "hnsw",
                "engine": "lucene",
                "space_type": d.space_type,
                "parameters": { "m": HNSW_M, "ef_construction": HNSW_EF_CONSTRUCTION }
            }
        }),
    );
    m.insert(src_hash_field(&field), json!({ "type": "keyword" }));
    m.insert(at_field(&field), json!({ "type": "date" }));
    m
}

fn as_object(meta: Option<&Value>) -> Map<String, Value> {
    meta.and_then(Value::as_object).cloned().unwrap_or_default()
}

fn slot_error(msg: String) -> Error {
    Error::Slot(msg)
}

/// `_meta` with the slot registered. The first slot registered becomes the default; later ones
/// only when `make_default` is set.
pub fn meta_with_slot(meta: Option<&Value>, d: &Descriptor, make_default: bool) -> Value {
    let mut obj = as_object(meta);
    let slot = d.slot_id();
    let slots = obj.entry(META_SLOTS).or_insert_with(|| json!({}));
    if !slots.is_object() {
        *slots = json!({});
    }
    slots[&slot] = serde_json::to_value(d).expect("descriptor serializes");
    if make_default || !obj.get(META_DEFAULT).is_some_and(Value::is_string) {
        obj.insert(META_DEFAULT.into(), Value::String(slot));
    }
    Value::Object(obj)
}

/// `_meta` with `slot` as the default. The slot must be registered.
pub fn meta_with_default(meta: Option<&Value>, slot: &str) -> Result<Value> {
    let mut obj = as_object(meta);
    if !registered(&obj).contains(&slot.to_string()) {
        return Err(slot_error(format!(
            "slot {slot} is not registered in _meta.{META_SLOTS}"
        )));
    }
    obj.insert(META_DEFAULT.into(), Value::String(slot.to_string()));
    Ok(Value::Object(obj))
}

/// `_meta` without `slot`. Refuses to remove the default: point the default elsewhere first.
pub fn meta_without_slot(meta: Option<&Value>, slot: &str) -> Result<Value> {
    let mut obj = as_object(meta);
    if default_slot(&Value::Object(obj.clone())).as_deref() == Some(slot) {
        return Err(slot_error(format!(
            "slot {slot} is the default; run set-default-slot for another slot first"
        )));
    }
    let Some(slots) = obj.get_mut(META_SLOTS).and_then(Value::as_object_mut) else {
        return Err(slot_error(format!(
            "slot {slot} is not registered (no _meta.{META_SLOTS})"
        )));
    };
    if slots.remove(slot).is_none() {
        return Err(slot_error(format!(
            "slot {slot} is not registered in _meta.{META_SLOTS}"
        )));
    }
    Ok(Value::Object(obj))
}

fn registered(obj: &Map<String, Value>) -> Vec<String> {
    obj.get(META_SLOTS)
        .and_then(Value::as_object)
        .map(|s| s.keys().cloned().collect())
        .unwrap_or_default()
}

pub fn default_slot(meta: &Value) -> Option<String> {
    meta.get(META_DEFAULT)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The descriptor registered under `slot`, verified to hash to that id.
pub fn slot_descriptor(meta: &Value, slot: &str) -> Result<Descriptor> {
    let raw = meta
        .get(META_SLOTS)
        .and_then(|s| s.get(slot))
        .ok_or_else(|| {
            slot_error(format!(
                "slot {slot} is not registered in _meta.{META_SLOTS}"
            ))
        })?;
    let d: Descriptor = serde_json::from_value(raw.clone()).map_err(|e| {
        slot_error(format!(
            "_meta.{META_SLOTS}.{slot} does not parse as a descriptor: {e}"
        ))
    })?;
    if d.slot_id() != slot {
        return Err(slot_error(format!(
            "_meta.{META_SLOTS}.{slot}: descriptor hashes to {}, not {slot}",
            d.slot_id()
        )));
    }
    Ok(d)
}

/// Every registered slot with its parsed descriptor. A descriptor that no longer parses is an
/// error: the index would be claiming a slot nobody can serve.
pub fn slots(meta: &Value) -> Result<Vec<(String, Descriptor)>> {
    let mut out = Vec::new();
    if let Some(slots) = meta.get(META_SLOTS).and_then(Value::as_object) {
        for id in slots.keys() {
            out.push((id.clone(), slot_descriptor(meta, id)?));
        }
    }
    Ok(out)
}

/// Slots that parse, and slots that do not (with the reason).
pub type SlotListing = (Vec<(String, Descriptor)>, Vec<(String, String)>);

/// Every registered slot, split into the ones that parse and the ones that do not (with the
/// reason). For operator tooling: a broken entry is reported, not fatal.
pub fn slots_lenient(meta: &Value) -> SlotListing {
    let mut ok = Vec::new();
    let mut broken = Vec::new();
    if let Some(slots) = meta.get(META_SLOTS).and_then(Value::as_object) {
        for id in slots.keys() {
            match slot_descriptor(meta, id) {
                Ok(d) => ok.push((id.clone(), d)),
                Err(e) => broken.push((id.clone(), e.to_string())),
            }
        }
    }
    (ok, broken)
}

/// One pipeline per alias, so staging and production never share one.
pub fn pipeline_name(index_alias: &str) -> String {
    format!("{index_alias}_hybrid_minmax")
}

/// Min-max normalization of each sub-query's scores, then the arithmetic mean: the hybrid mode
/// fusion from the design. Weights are equal; the API can override per request later.
pub fn hybrid_pipeline_body() -> Value {
    json!({
        "description": "Semantic search hybrid mode: min-max normalized lexical + k-NN scores, arithmetic mean",
        "phase_results_processors": [{
            "normalization-processor": {
                "normalization": { "technique": "min_max" },
                "combination": { "technique": "arithmetic_mean", "parameters": { "weights": [0.5, 0.5] } }
            }
        }]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::test_support::sample;

    fn descriptor(dims: usize) -> Descriptor {
        let mut d = sample();
        d.dimensions = dims;
        d
    }

    #[test]
    fn field_mappings_are_named_by_slot_and_sized_by_descriptor() {
        let d = descriptor(384);
        let m = slot_field_mappings(&d);
        let f = d.vector_field();
        assert_eq!(m[&f]["type"], "knn_vector");
        assert_eq!(m[&f]["dimension"], 384);
        assert_eq!(m[&f]["method"]["engine"], "lucene");
        assert_eq!(m[&f]["method"]["space_type"], "cosinesimil");
        assert_eq!(m[&src_hash_field(&f)]["type"], "keyword");
        assert_eq!(m[&at_field(&f)]["type"], "date");
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn first_slot_becomes_default_later_ones_only_on_request() {
        let a = descriptor(384);
        let b = descriptor(768);
        let meta = meta_with_slot(None, &a, false);
        assert_eq!(default_slot(&meta).as_deref(), Some(a.slot_id().as_str()));
        let meta = meta_with_slot(Some(&meta), &b, false);
        assert_eq!(default_slot(&meta).as_deref(), Some(a.slot_id().as_str()));
        assert_eq!(slots(&meta).unwrap().len(), 2);
        assert_eq!(slot_descriptor(&meta, &b.slot_id()).unwrap(), b);
        let meta = meta_with_slot(Some(&meta), &b, true);
        assert_eq!(default_slot(&meta).as_deref(), Some(b.slot_id().as_str()));
        let meta = meta_with_slot(Some(&json!({"owner": "search"})), &a, false);
        assert_eq!(meta["owner"], "search");
    }

    #[test]
    fn default_and_retire_rules() {
        let a = descriptor(384);
        let b = descriptor(768);
        let meta = meta_with_slot(Some(&meta_with_slot(None, &a, false)), &b, false);
        assert!(meta_with_default(Some(&meta), "0000000000").is_err());
        let meta = meta_with_default(Some(&meta), &b.slot_id()).unwrap();
        assert_eq!(default_slot(&meta).as_deref(), Some(b.slot_id().as_str()));
        assert!(
            meta_without_slot(Some(&meta), &b.slot_id()).is_err(),
            "cannot retire the default"
        );
        let meta = meta_without_slot(Some(&meta), &a.slot_id()).unwrap();
        assert_eq!(slots(&meta).unwrap().len(), 1);
        assert!(
            meta_without_slot(Some(&meta), &a.slot_id()).is_err(),
            "already gone"
        );
    }

    #[test]
    fn slots_refuses_a_descriptor_filed_under_the_wrong_id() {
        let a = descriptor(384);
        let meta = json!({ META_SLOTS: { "0123456789": serde_json::to_value(&a).unwrap() } });
        assert!(slots(&meta).is_err());
        assert!(slot_descriptor(&meta, "0123456789").is_err());
        let (ok, broken) = slots_lenient(&meta);
        assert!(ok.is_empty());
        assert_eq!(broken.len(), 1);
        assert_eq!(broken[0].0, "0123456789");
    }

    #[test]
    fn slots_lenient_reports_unparsable_entries_beside_good_ones() {
        let a = descriptor(384);
        let meta = json!({ META_SLOTS: {
            a.slot_id(): serde_json::to_value(&a).unwrap(),
            "deadbeef00": { "note": "hand-written, no model_file" }
        } });
        let (ok, broken) = slots_lenient(&meta);
        assert_eq!(ok.len(), 1);
        assert_eq!(broken.len(), 1);
        assert!(broken[0].1.contains("does not parse"));
    }

    #[test]
    fn pipeline_is_per_alias() {
        assert_eq!(
            pipeline_name("testnet_entities"),
            "testnet_entities_hybrid_minmax"
        );
        assert_eq!(pipeline_name("entities"), "entities_hybrid_minmax");
        let body = hybrid_pipeline_body();
        assert_eq!(
            body["phase_results_processors"][0]["normalization-processor"]["normalization"]["technique"],
            "min_max"
        );
    }
}
