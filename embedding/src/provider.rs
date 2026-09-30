//! The provider contract: one model behind a descriptor.

use serde::{Deserialize, Serialize};

use crate::descriptor::Descriptor;
use crate::error::Result;

/// Whether a text is a stored document or a search query. Models with asymmetric prompts
/// (e5, nomic, EmbeddingGemma) embed the two differently; the descriptor carries both prompts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Purpose {
    Document,
    Query,
}

/// Embeds texts for exactly one descriptor. Implementations are blocking and CPU-bound; async
/// callers wrap them in `spawn_blocking`.
pub trait EmbeddingProvider: Send + Sync {
    fn descriptor(&self) -> &Descriptor;

    /// One vector per input text, in order, each with `descriptor().dimensions` components,
    /// L2-normalized. The prompt for `purpose` is applied inside; callers pass raw text.
    fn embed(&self, texts: &[String], purpose: Purpose) -> Result<Vec<Vec<f32>>>;
}
