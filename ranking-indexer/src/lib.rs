//! ranking-indexer: consumes `knowledge.edits` and `space.membership`,
//! maintains the private `ranks` working schema (including its own view of
//! space membership), and projects aggregated `RANK_POSITION` relations back
//! into the public graph.

pub mod account_exclusions_compute;
pub mod account_exclusions_mirror;
pub mod account_vote_weight;
pub mod account_weights_compute;
pub mod account_weights_mirror;
pub mod agenda_listings_mirror;
pub mod anchor_select;
pub mod clickhouse;
pub mod consumer;
pub mod dedup;
pub mod derived_mirror;
pub mod detect;
pub mod eligibility;
pub mod error;
pub mod external_interest_mirror;
pub mod feed_composition_mirror;
pub mod feed_signals;
pub mod feed_signals_compute;
pub mod feed_signals_mirror;
pub mod feed_user_seen_mirror;
pub mod membership;
pub mod metrics;
pub mod models;
pub mod not_interested;
pub mod primer_claims_mirror;
pub mod primer_mirror;
pub mod primer_stats;
pub mod publish;
pub mod recompute;
pub mod scoring;
pub mod sql_helpers;
pub mod stance_graph;
pub mod stance_map;
pub mod storage;
pub mod topic_cooccurrence_compute;
pub mod topic_cooccurrence_mirror;
pub mod topic_interest;
pub mod topic_suggestions_compute;
pub mod topic_suggestions_mirror;
pub mod user_interest;
pub mod user_interest_shadow;
pub mod user_topic_signals_mirror;
pub mod wallet_hash;
