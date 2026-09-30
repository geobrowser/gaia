//! Shared embedding model for gaia's semantic search.
//!
//! Three things live here so that every process that touches a vector agrees on what it is:
//!
//! - [`Descriptor`]: the immutable record of *which model, over which text, with which settings*.
//!   Its canonical hash is the slot id; the OpenSearch field for a slot is `emb_<slot>`.
//! - [`template`]: the text template that turns a search document into the string that gets
//!   embedded. It is part of the descriptor because two deployments of the same weights over
//!   different text produce incompatible spaces.
//! - [`EmbeddingProvider`] and its local implementation [`OnnxLocalProvider`], built from a
//!   hash-verified [`bundle`] on disk.
//!
//! Design: `docs/tech-designs/semantic-search.md`.

pub mod bundle;
pub mod descriptor;
pub mod error;
pub mod onnx;
pub mod provider;
pub mod template;

pub use bundle::VerifiedBundle;
pub use descriptor::{Descriptor, Pooling, Quantization};
pub use error::{Error, Result};
pub use onnx::{BundleFiles, OnnxLocalProvider, OnnxOptions};
pub use provider::{EmbeddingProvider, Purpose};
