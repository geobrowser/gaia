//! Re-export of the pure slot rules, which live in the `embedding` crate so the embedding-indexer
//! and tests read the same field names and `_meta` layout as this tool writes.
pub use embedding::slots::*;
