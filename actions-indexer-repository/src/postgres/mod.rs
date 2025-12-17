//! PostgreSQL implementation of the actions indexer repository.
//!
//! Provides a production-ready PostgreSQL backend for the `ActionsRepository` trait
//! with connection pooling, transaction safety, and batch operations.
//!
//! ## Key Features
//!
//! - Connection pooling with `sqlx::PgPool`
//! - ACID transactions with automatic rollback
//! - Bulk operations using PostgreSQL's `UNNEST` and `VALUES`
//! - Upsert support with `ON CONFLICT DO UPDATE`
//! - Type-safe queries with SQLx
//!
//! ## Database Tables
//!
//! - `raw_actions`: Processed blockchain actions
//! - `user_votes`: Individual voting records with upsert support
//! - `votes_count`: Aggregated vote tallies per entity/space
mod actions_repository;
mod cursor_repository;
pub use actions_repository::PostgresActionsRepository;
pub use cursor_repository::PostgresCursorRepository;
