//! embedding-service: the one in-cluster embedding runtime.
//!
//! Loads hash-verified bundles from a models directory and serves them:
//!
//! - `POST /embed` `{ slot, purpose, texts }` → vectors for exactly that slot
//! - `GET /info` → every loaded slot's descriptor and hash, so callers can verify they run the
//!   same model as the index they write to or read from
//! - `GET /health/live`, `GET /health/ready`
//!
//! Both the search API (queries) and the embedding-indexer (documents) call it; nothing else in
//! gaia embeds text. Design: `docs/tech-designs/semantic-search.md`.

pub mod bundle_cmd;
pub mod config;
pub mod server;
