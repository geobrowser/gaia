//! embedding-indexer: the writer side of semantic search.
//!
//! One process per embedding slot. It reads the search view — never the Kafka edit stream — and
//! keeps the invariant that every in-scope document's `emb_<slot>` is the embedding of that
//! document's current text (`embedding::template`), with `emb_<slot>_src_hash` holding the hash
//! of that text. Documents out of scope carry no slot fields.
//!
//! Two modes share one loop: a **backfill** (full scan, resumable by a persisted sort key) on first
//! run, then **follow** (documents whose `indexed_at` moved since the last checkpoint, with an
//! overlap window). Vectors are written with a compare-and-set script that no-ops when the text
//! changed between read and write; the next poll picks the document up again.
//!
//! Design: `docs/tech-designs/semantic-search.md` (D4, D5, D11).

pub mod config;
pub mod engine;
pub mod error;
pub mod health;
pub mod queries;
pub mod scope;
pub mod service;
pub mod store;

pub use config::Config;
pub use engine::{CycleStats, Engine};
pub use error::{IndexerError, Result};
