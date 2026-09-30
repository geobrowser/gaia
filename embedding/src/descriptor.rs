//! The embedding descriptor: everything that defines a vector space, pinned by content.
//!
//! A descriptor is immutable. Change any field and you have a new slot, a new OpenSearch field
//! and a new backfill. Model *artifacts* are pinned by SHA-256, not by name: a model name is a
//! label (fastembed's default for "bge-small-en-v1.5" is a quantized export from a mirror, not
//! the original file), a hash is an identity.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::provider::Purpose;
use crate::template;

/// Length of a slot id: the first 10 hex characters of the descriptor hash.
pub const SLOT_ID_LEN: usize = 10;

/// Tokenizer files every bundle must carry (what the ONNX provider needs beside the model).
pub const TOKENIZER_FILES: [&str; 4] = [
    "tokenizer.json",
    "config.json",
    "special_tokens_map.json",
    "tokenizer_config.json",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pooling {
    Cls,
    Mean,
}

/// Mirrors fastembed's quantization mode for the ONNX export: it decides how batches are formed
/// (dynamic-quantized exports are not batching-safe), so it is part of the model's identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quantization {
    None,
    Static,
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    /// Provider kind. v1 knows `onnx-local`.
    pub provider: String,
    /// Human name of the model. Documentation only; the artifacts are the identity.
    pub model_id: String,
    /// Where the artifacts come from: `hf:<repo>@<revision>`.
    pub source: String,
    /// The artifact holding the ONNX graph (a key of `artifacts`).
    pub model_file: String,
    /// Every file in the bundle with its `sha256:<hex>` digest.
    pub artifacts: BTreeMap<String, String>,
    pub dimensions: usize,
    pub pooling: Pooling,
    pub quantization: Quantization,
    /// v1 providers always L2-normalize; the field exists so a future one can say otherwise.
    pub normalize: bool,
    /// Token limit the tokenizer truncates to.
    pub max_tokens: usize,
    /// How over-long text is cut. v1: `tail`.
    pub truncation: String,
    /// Name of the [`template`] that turns a document into the embedded text.
    pub text_template: String,
    /// Prefix applied to documents before embedding (empty for bge-small).
    #[serde(default)]
    pub document_prompt: String,
    /// Prefix applied to queries before embedding (empty for bge-small; e5/nomic/gemma need one).
    #[serde(default)]
    pub query_prompt: String,
    /// OpenSearch k-NN space type the vectors are meant for.
    pub space_type: String,
    /// Default minimum score for this slot on the `(1 + cos) / 2` scale; measured, never copied.
    pub score_floor: f64,
}

impl Descriptor {
    /// Structural checks a descriptor must pass before anything is built from it.
    pub fn validate(&self) -> Result<()> {
        let bad = |m: String| Err(Error::InvalidDescriptor(m));
        if self.provider != "onnx-local" {
            return bad(format!("unsupported provider {:?}", self.provider));
        }
        if self.dimensions == 0 {
            return bad("dimensions must be > 0".into());
        }
        if self.max_tokens == 0 {
            return bad("max_tokens must be > 0".into());
        }
        if !self.normalize {
            return bad("normalize must be true (v1 providers always L2-normalize)".into());
        }
        if self.truncation != "tail" {
            return bad(format!("unsupported truncation {:?}", self.truncation));
        }
        if !template::is_known(&self.text_template) {
            return Err(Error::UnknownTemplate(self.text_template.clone()));
        }
        if self.space_type != "cosinesimil" {
            return bad(format!("unsupported space_type {:?}", self.space_type));
        }
        if !(0.0..=1.0).contains(&self.score_floor) {
            return bad("score_floor must be within [0, 1]".into());
        }
        if !self.artifacts.contains_key(&self.model_file) {
            return bad(format!(
                "model_file {:?} is not listed in artifacts",
                self.model_file
            ));
        }
        for f in TOKENIZER_FILES {
            if !self.artifacts.contains_key(f) {
                return bad(format!("artifacts must include {f}"));
            }
        }
        for (file, digest) in &self.artifacts {
            match digest.strip_prefix("sha256:") {
                Some(hex) if hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) => {}
                _ => {
                    return bad(format!(
                        "artifact {file}: digest must be \"sha256:<64 hex>\""
                    ));
                }
            }
        }
        Ok(())
    }

    /// Canonical JSON: keys sorted at every level, no whitespace. Hashing this is what makes the
    /// slot id independent of field order and formatting.
    pub fn canonical_json(&self) -> String {
        let value = serde_json::to_value(self).expect("a descriptor always serializes");
        let mut out = String::new();
        canonicalize(&value, &mut out);
        out
    }

    /// Full SHA-256 of the canonical JSON, hex. Compared field-for-field-equivalent by services
    /// verifying they run the same slot.
    pub fn hash(&self) -> String {
        hex::encode(Sha256::digest(self.canonical_json().as_bytes()))
    }

    /// Short id used in field names and directory names: the first [`SLOT_ID_LEN`] hex chars.
    pub fn slot_id(&self) -> String {
        self.hash()[..SLOT_ID_LEN].to_string()
    }

    /// The OpenSearch `knn_vector` field for this slot.
    pub fn vector_field(&self) -> String {
        format!("emb_{}", self.slot_id())
    }

    /// The `sha256:<hex>` digest recorded for an artifact.
    pub fn artifact_digest(&self, file: &str) -> Option<&str> {
        self.artifacts.get(file).map(String::as_str)
    }

    /// The prompt a provider prepends for this purpose.
    pub fn prompt(&self, purpose: Purpose) -> &str {
        match purpose {
            Purpose::Document => &self.document_prompt,
            Purpose::Query => &self.query_prompt,
        }
    }
}

/// Serialize a JSON value with object keys sorted, recursively, and no whitespace.
pub fn canonicalize(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).expect("string serializes"));
                out.push(':');
                canonicalize(&map[*k], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonicalize(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&serde_json::to_string(scalar).expect("scalar serializes")),
    }
}

/// Whether a string has the shape of a slot id (lowercase hex of [`SLOT_ID_LEN`]).
pub fn is_slot_id(s: &str) -> bool {
    s.len() == SLOT_ID_LEN
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// A valid descriptor with made-up digests, for tests that never load a model.
    pub fn sample() -> Descriptor {
        let d = |n: u8| format!("sha256:{}", format!("{n:02x}").repeat(32));
        Descriptor {
            provider: "onnx-local".into(),
            model_id: "test/model".into(),
            source: "hf:test/model@0123456789abcdef0123456789abcdef01234567".into(),
            model_file: "model.onnx".into(),
            artifacts: BTreeMap::from([
                ("model.onnx".to_string(), d(1)),
                ("tokenizer.json".to_string(), d(2)),
                ("config.json".to_string(), d(3)),
                ("special_tokens_map.json".to_string(), d(4)),
                ("tokenizer_config.json".to_string(), d(5)),
            ]),
            dimensions: 4,
            pooling: Pooling::Cls,
            quantization: Quantization::Static,
            normalize: true,
            max_tokens: 512,
            truncation: "tail".into(),
            text_template: template::NAME_DESCRIPTION_V1.into(),
            document_prompt: String::new(),
            query_prompt: String::new(),
            space_type: "cosinesimil".into(),
            score_floor: 0.85,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::sample;
    use super::*;

    #[test]
    fn slot_id_is_deterministic_and_field_order_independent() {
        let d = sample();
        assert!(is_slot_id(&d.slot_id()));
        assert_eq!(d.slot_id(), sample().slot_id());
        // Same content through a differently ordered JSON text → same slot.
        let json = serde_json::to_string(&d).unwrap();
        let reparsed: Descriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(reparsed.hash(), d.hash());
        assert_eq!(d.vector_field(), format!("emb_{}", d.slot_id()));
    }

    #[test]
    fn any_field_change_is_a_new_slot() {
        let base = sample();
        let mut prompt = base.clone();
        prompt.query_prompt = "query: ".into();
        let mut dims = base.clone();
        dims.dimensions = 8;
        let mut file = base.clone();
        file.artifacts
            .insert("model.onnx".into(), format!("sha256:{}", "ff".repeat(32)));
        for changed in [prompt, dims, file] {
            assert_ne!(changed.slot_id(), base.slot_id());
        }
    }

    #[test]
    fn canonical_json_sorts_keys_recursively() {
        let v: Value =
            serde_json::from_str(r#"{"b":{"z":1,"a":[{"y":2,"x":1}]},"a":true}"#).unwrap();
        let mut out = String::new();
        canonicalize(&v, &mut out);
        assert_eq!(out, r#"{"a":true,"b":{"a":[{"x":1,"y":2}],"z":1}}"#);
    }

    #[test]
    fn validate_rejects_the_obvious() {
        assert!(sample().validate().is_ok());
        let mut d = sample();
        d.artifacts.remove("tokenizer.json");
        assert!(matches!(d.validate(), Err(Error::InvalidDescriptor(_))));
        let mut d = sample();
        d.artifacts.insert("model.onnx".into(), "md5:abc".into());
        assert!(d.validate().is_err());
        let mut d = sample();
        d.text_template = "nope".into();
        assert!(matches!(d.validate(), Err(Error::UnknownTemplate(_))));
        let mut d = sample();
        d.normalize = false;
        assert!(d.validate().is_err());
    }

    #[test]
    fn unknown_fields_are_rejected_on_parse() {
        let mut v = serde_json::to_value(sample()).unwrap();
        v["surprise"] = Value::Bool(true);
        assert!(serde_json::from_value::<Descriptor>(v).is_err());
    }
}
