//! Fits and evaluates the stance map in SHADOW (GEO-3146, migration 0108).
//!
//! Once a day, from the `ranking-indexer-stance-map-shadow` CronJob:
//!
//! 1. reads the observations (`personalization.stance_map_observations()`: weighted stance votes on
//!    claims, plus debaters' stated sides) and the claim-stance seeds
//!    (`personalization.stance_map_seeds()`);
//! 2. fits the map and measures it (`ranking_indexer::stance_map::evaluate`): held-out AUC against
//!    tendencies only, with and without the seeds, and half-split stability of the main axis;
//! 3. records one row through `personalization.record_stance_map_run()`, which decides the gate.
//!
//! Nothing reads the map. Per-user positions exist only in this process's memory and are dropped
//! when it exits: they are never written and never logged. Only aggregate metrics leave.
//!
//! Usage: `DATABASE_URL=... cargo run --release --bin stance_map_shadow`

use std::collections::HashMap;
use std::env;
use std::time::Instant;

use sqlx::postgres::PgPoolOptions;
use tracing::info;
use uuid::Uuid;

use ranking_indexer::error::IndexerError;
use ranking_indexer::stance_map::{evaluate, Dataset, EvalConfig, Obs, Seed};

/// Dense indices for one run. Dropped with the dataset.
#[derive(Default)]
struct Index(HashMap<Uuid, usize>);

impl Index {
    fn of(&mut self, id: Uuid) -> usize {
        let n = self.0.len();
        *self.0.entry(id).or_insert(n)
    }
    fn get(&self, id: &Uuid) -> Option<usize> {
        self.0.get(id).copied()
    }
}

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| IndexerError::Config("DATABASE_URL not set".into()))?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;

    let started = Instant::now();
    let rows: Vec<(Uuid, Uuid, bool, f64, String)> = sqlx::query_as(
        "SELECT user_id, claim_id, agree, weight, source FROM personalization.stance_map_observations()",
    )
    .fetch_all(&pool)
    .await?;
    let seed_rows: Vec<(Uuid, Uuid, i16)> = sqlx::query_as(
        "SELECT claim_id, main_claim_id, sign FROM personalization.stance_map_seeds()",
    )
    .fetch_all(&pool)
    .await?;

    let mut users = Index::default();
    let mut claims = Index::default();
    let obs: Vec<Obs> = rows
        .into_iter()
        .map(|(u, c, agree, weight, source)| Obs {
            user: users.of(u),
            claim: claims.of(c),
            agree,
            weight,
            stated: source == "stated",
        })
        .collect();
    // A seed matters only where both claims are in the map; a seeded claim nobody voted on adds
    // nothing to any measurement, so it is not given a position.
    let seeds: Vec<Seed> = seed_rows
        .into_iter()
        .filter_map(|(c, m, sign)| {
            Some(Seed {
                claim: claims.get(&c)?,
                main: claims.get(&m)?,
                sign: f64::from(sign.signum()),
            })
        })
        .collect();
    let data = Dataset {
        n_users: users.0.len(),
        n_claims: claims.0.len(),
        obs,
        seeds,
    };
    drop(users);
    drop(claims);

    let eval = evaluate(&data, &EvalConfig::default());
    drop(data);

    let mut metrics = serde_json::to_value(&eval)
        .map_err(|e| IndexerError::Config(format!("serialising metrics: {e}")))?;
    metrics["elapsed_ms"] = serde_json::json!(started.elapsed().as_millis() as u64);
    let gate_met: bool =
        sqlx::query_scalar("SELECT personalization.record_stance_map_run($1::jsonb)")
            .bind(&metrics)
            .fetch_one(&pool)
            .await?;

    // Aggregates only. Never log a user id or a position.
    info!(
        clean_votes = eval.clean_votes,
        voters = eval.voters,
        claims = eval.claims,
        stated = eval.stated_positions,
        seeded = eval.seeded_claims,
        axes = ?eval.axes,
        auc_map = ?eval.auc_map,
        auc_tendencies = ?eval.auc_tendencies,
        auc_lift = ?eval.auc_lift,
        split_correlation = ?eval.split_correlation,
        gate_met,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "stance map evaluated (shadow)"
    );
    Ok(())
}
