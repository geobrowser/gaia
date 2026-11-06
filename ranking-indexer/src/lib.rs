//! ranking-indexer: consumes `knowledge.edits` and `space.membership`,
//! maintains the private `ranks` working schema (including its own view of
//! space membership), and projects aggregated `RANK_POSITION` relations back
//! into the public graph.

pub mod account_exclusions_mirror;
pub mod account_weights_mirror;
pub mod agenda_listings_mirror;
pub mod clickhouse;
pub mod consumer;
pub mod dedup;
pub mod derived_mirror;
pub mod detect;
pub mod eligibility;
pub mod error;
pub mod feed_composition_mirror;
pub mod feed_signals;
pub mod feed_signals_mirror;
pub mod membership;
pub mod metrics;
pub mod models;
pub mod not_interested;
pub mod primer_claims_mirror;
pub mod primer_mirror;
pub mod publish;
pub mod recompute;
pub mod scoring;
pub mod stance_map;
pub mod storage;
pub mod topic_cooccurrence_mirror;
pub mod topic_interest;
pub mod topic_suggestions_mirror;
