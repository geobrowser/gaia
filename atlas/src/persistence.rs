use std::collections::HashSet;
use std::time::Duration;

use crate::events::{SpaceId, TopicId};
use crate::graph::{EdgeType, GraphState};
use hermes_instrumentation::{error, info, warn};
use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgPoolOptions, PgPool, Row};

const CHECKPOINT_SCHEMA_VERSION: u32 = 1;
const FAIL_OPEN_BOUND_DEFAULT: u64 = 10;
const FAIL_OPEN_BOUND_MIN: u64 = 1;
const FAIL_OPEN_BOUND_MAX: u64 = 10_000;
const RETRY_ATTEMPTS_DEFAULT: u32 = 3;
const RETRY_ATTEMPTS_MIN: u32 = 1;
const RETRY_ATTEMPTS_MAX: u32 = 10;
const RETRY_BACKOFF_MS_DEFAULT: u64 = 200;
const RETRY_BACKOFF_MS_MIN: u64 = 50;
const RETRY_BACKOFF_MS_MAX: u64 = 30_000;
const PAUSE_RECOVERY_MAX_ATTEMPTS_DEFAULT: u32 = 120;
const PAUSE_RECOVERY_MAX_ATTEMPTS_MIN: u32 = 1;
const PAUSE_RECOVERY_MAX_ATTEMPTS_MAX: u32 = 10_000;
const PG_POOL_MAX_CONNECTIONS_DEFAULT: u32 = 2;
const PG_POOL_MAX_CONNECTIONS_MIN: u32 = 1;
const PG_POOL_MAX_CONNECTIONS_MAX: u32 = 10;
const PG_POOL_MIN_CONNECTIONS_DEFAULT: u32 = 0;
const PG_POOL_MIN_CONNECTIONS_MIN: u32 = 0;
const PG_POOL_MIN_CONNECTIONS_MAX: u32 = 5;
const PG_POOL_ACQUIRE_TIMEOUT_MS_DEFAULT: u64 = 5_000;
const PG_POOL_ACQUIRE_TIMEOUT_MS_MIN: u64 = 100;
const PG_POOL_ACQUIRE_TIMEOUT_MS_MAX: u64 = 60_000;
const PG_POOL_IDLE_TIMEOUT_MS_DEFAULT: u64 = 60_000;
const PG_POOL_IDLE_TIMEOUT_MS_MIN: u64 = 1_000;
const PG_POOL_IDLE_TIMEOUT_MS_MAX: u64 = 600_000;
const PG_POOL_MAX_LIFETIME_MS_DEFAULT: u64 = 1_800_000;
const PG_POOL_MAX_LIFETIME_MS_MIN: u64 = 60_000;
const PG_POOL_MAX_LIFETIME_MS_MAX: u64 = 7_200_000;
const PG_STATEMENT_TIMEOUT_MS_DEFAULT: u64 = 3_000;
const PG_STATEMENT_TIMEOUT_MS_MIN: u64 = 100;
const PG_STATEMENT_TIMEOUT_MS_MAX: u64 = 60_000;

#[derive(Debug, Clone)]
pub struct PostgresPoolConfig {
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout_ms: u64,
    pub idle_timeout_ms: u64,
    pub max_lifetime_ms: u64,
    pub statement_timeout_ms: u64,
}

#[derive(Debug, Clone)]
pub struct CheckpointConfig {
    pub root_space_id: SpaceId,
    pub graph_state_version: u32,
    pub indexer_id: String,
    pub runtime_compatibility_marker: String,
    pub fail_open_bound: u64,
    pub checkpoint_retry_attempts: u32,
    pub checkpoint_retry_backoff_ms: u64,
    pub pause_recovery_max_attempts: u32,
    pub allow_fresh_start_on_invalid_checkpoint: bool,
    pub store: Option<PostgresCheckpointStore>,
}

impl CheckpointConfig {
    pub fn from_env(
        root_space_id: SpaceId,
        graph_state_version: u32,
    ) -> Result<Self, CheckpointError> {
        let database_url = std::env::var("ATLAS_CHECKPOINT_DATABASE_URL").ok();
        let indexer_id_env = std::env::var("ATLAS_INDEXER_ID")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        let pool_config = PostgresPoolConfig {
            max_connections: parse_clamped_env_u32(
                "ATLAS_CHECKPOINT_POOL_MAX_CONNECTIONS",
                PG_POOL_MAX_CONNECTIONS_DEFAULT,
                PG_POOL_MAX_CONNECTIONS_MIN,
                PG_POOL_MAX_CONNECTIONS_MAX,
                "checkpoint pool max connections",
            ),
            min_connections: parse_clamped_env_u32(
                "ATLAS_CHECKPOINT_POOL_MIN_CONNECTIONS",
                PG_POOL_MIN_CONNECTIONS_DEFAULT,
                PG_POOL_MIN_CONNECTIONS_MIN,
                PG_POOL_MIN_CONNECTIONS_MAX,
                "checkpoint pool min connections",
            ),
            acquire_timeout_ms: parse_clamped_env_u64(
                "ATLAS_CHECKPOINT_POOL_ACQUIRE_TIMEOUT_MS",
                PG_POOL_ACQUIRE_TIMEOUT_MS_DEFAULT,
                PG_POOL_ACQUIRE_TIMEOUT_MS_MIN,
                PG_POOL_ACQUIRE_TIMEOUT_MS_MAX,
                "checkpoint pool acquire timeout",
            ),
            idle_timeout_ms: parse_clamped_env_u64(
                "ATLAS_CHECKPOINT_POOL_IDLE_TIMEOUT_MS",
                PG_POOL_IDLE_TIMEOUT_MS_DEFAULT,
                PG_POOL_IDLE_TIMEOUT_MS_MIN,
                PG_POOL_IDLE_TIMEOUT_MS_MAX,
                "checkpoint pool idle timeout",
            ),
            max_lifetime_ms: parse_clamped_env_u64(
                "ATLAS_CHECKPOINT_POOL_MAX_LIFETIME_MS",
                PG_POOL_MAX_LIFETIME_MS_DEFAULT,
                PG_POOL_MAX_LIFETIME_MS_MIN,
                PG_POOL_MAX_LIFETIME_MS_MAX,
                "checkpoint pool max lifetime",
            ),
            statement_timeout_ms: parse_clamped_env_u64(
                "ATLAS_CHECKPOINT_STATEMENT_TIMEOUT_MS",
                PG_STATEMENT_TIMEOUT_MS_DEFAULT,
                PG_STATEMENT_TIMEOUT_MS_MIN,
                PG_STATEMENT_TIMEOUT_MS_MAX,
                "checkpoint statement timeout",
            ),
        };

        let store = match database_url {
            Some(url) => Some(PostgresCheckpointStore::new(&url, &pool_config)?),
            None => None,
        };

        let indexer_id = match (store.is_some(), indexer_id_env) {
            (true, Some(value)) => value,
            (true, None) => {
                return Err(CheckpointError::Incompatible(
                    "ATLAS_INDEXER_ID must be set and non-empty when checkpoint persistence is enabled"
                        .to_string(),
                ));
            }
            (false, Some(value)) => value,
            (false, None) => "atlas-default".to_string(),
        };

        let retry_attempts = parse_clamped_env_u32(
            "ATLAS_CHECKPOINT_RETRY_ATTEMPTS",
            RETRY_ATTEMPTS_DEFAULT,
            RETRY_ATTEMPTS_MIN,
            RETRY_ATTEMPTS_MAX,
            "checkpoint retry attempts",
        );

        let fail_open_bound = parse_clamped_env_u64(
            "ATLAS_FAIL_OPEN_BOUND",
            FAIL_OPEN_BOUND_DEFAULT,
            FAIL_OPEN_BOUND_MIN,
            FAIL_OPEN_BOUND_MAX,
            "fail-open bound",
        );

        let retry_backoff_ms = parse_clamped_env_u64(
            "ATLAS_CHECKPOINT_RETRY_BACKOFF_MS",
            RETRY_BACKOFF_MS_DEFAULT,
            RETRY_BACKOFF_MS_MIN,
            RETRY_BACKOFF_MS_MAX,
            "checkpoint retry backoff",
        );

        let pause_recovery_max_attempts = parse_clamped_env_u32(
            "ATLAS_PAUSE_RECOVERY_MAX_ATTEMPTS",
            PAUSE_RECOVERY_MAX_ATTEMPTS_DEFAULT,
            PAUSE_RECOVERY_MAX_ATTEMPTS_MIN,
            PAUSE_RECOVERY_MAX_ATTEMPTS_MAX,
            "pause recovery max attempts",
        );

        let allow_fresh_start_on_invalid_checkpoint =
            parse_env_bool("ATLAS_CHECKPOINT_ALLOW_FRESH_START", false);

        Ok(Self {
            root_space_id,
            graph_state_version,
            indexer_id,
            runtime_compatibility_marker: std::env::var("ATLAS_RUNTIME_COMPATIBILITY_MARKER")
                .unwrap_or_else(|_| "atlas-v2".to_string()),
            fail_open_bound,
            checkpoint_retry_attempts: retry_attempts,
            checkpoint_retry_backoff_ms: retry_backoff_ms,
            pause_recovery_max_attempts,
            allow_fresh_start_on_invalid_checkpoint,
            store,
        })
    }
}

#[derive(Debug)]
pub struct CheckpointManager {
    config: CheckpointConfig,
    fail_open: std::sync::Mutex<FailOpenState>,
    restored_cursor: Option<String>,
}

#[derive(Debug, Default)]
struct FailOpenState {
    consecutive_uncheckpointed_blocks: u64,
    pending_checkpoint: Option<Checkpoint>,
    paused: bool,
    pause_recovery_attempts: u32,
}

impl CheckpointManager {
    pub fn new(config: CheckpointConfig) -> Self {
        Self {
            config,
            fail_open: std::sync::Mutex::new(FailOpenState::default()),
            restored_cursor: None,
        }
    }

    pub fn from_env(
        root_space_id: SpaceId,
        graph_state_version: u32,
    ) -> Result<Self, CheckpointError> {
        Ok(Self::new(CheckpointConfig::from_env(
            root_space_id,
            graph_state_version,
        )?))
    }

    pub fn restored_cursor(&self) -> Option<String> {
        self.restored_cursor.clone()
    }

    pub async fn restore_checkpoint_on_startup(
        &mut self,
    ) -> Result<Option<GraphState>, CheckpointError> {
        let Some(store) = &self.config.store else {
            info!("Checkpoint persistence disabled (no ATLAS_CHECKPOINT_DATABASE_URL)");
            return Ok(None);
        };

        match store.load(&self.config.indexer_id).await {
            Ok(Some(checkpoint)) => {
                if let Err(err) = checkpoint.validate_compatibility(
                    &self.config.indexer_id,
                    &self.config.runtime_compatibility_marker,
                    self.config.root_space_id,
                    self.config.graph_state_version,
                ) {
                    if self.config.allow_fresh_start_on_invalid_checkpoint {
                        warn!(
                            indexer_id = %self.config.indexer_id,
                            root_space_id = %encode_id(self.config.root_space_id),
                            graph_state_version = self.config.graph_state_version,
                            reason = %err,
                            block_number = checkpoint.block_number,
                            cursor = %checkpoint.cursor,
                            "Checkpoint rejected; ATLAS_CHECKPOINT_ALLOW_FRESH_START enabled, starting fresh"
                        );
                        return Ok(None);
                    }

                    error!(
                        indexer_id = %self.config.indexer_id,
                        root_space_id = %encode_id(self.config.root_space_id),
                        graph_state_version = self.config.graph_state_version,
                        reason = %err,
                        block_number = checkpoint.block_number,
                        cursor = %checkpoint.cursor,
                        "Checkpoint rejected and fresh-start fallback is disabled"
                    );
                    return Err(err);
                }

                let graph_state = match checkpoint.graph_state() {
                    Ok(state) => state,
                    Err(err) => {
                        if self.config.allow_fresh_start_on_invalid_checkpoint {
                            warn!(
                                indexer_id = %self.config.indexer_id,
                                root_space_id = %encode_id(self.config.root_space_id),
                                graph_state_version = self.config.graph_state_version,
                                reason = %err,
                                block_number = checkpoint.block_number,
                                cursor = %checkpoint.cursor,
                                "Checkpoint graph state unreadable; ATLAS_CHECKPOINT_ALLOW_FRESH_START enabled, starting fresh"
                            );
                            return Ok(None);
                        }

                        error!(
                            indexer_id = %self.config.indexer_id,
                            root_space_id = %encode_id(self.config.root_space_id),
                            graph_state_version = self.config.graph_state_version,
                            reason = %err,
                            block_number = checkpoint.block_number,
                            cursor = %checkpoint.cursor,
                            "Checkpoint graph state unreadable and fresh-start fallback is disabled"
                        );
                        return Err(err);
                    }
                };

                self.restored_cursor = Some(checkpoint.cursor.clone());
                info!(
                    indexer_id = %self.config.indexer_id,
                    root_space_id = %encode_id(self.config.root_space_id),
                    graph_state_version = self.config.graph_state_version,
                    block_number = checkpoint.block_number,
                    cursor = %checkpoint.cursor,
                    "Checkpoint restored"
                );
                Ok(Some(graph_state))
            }
            Ok(None) => {
                info!(
                    indexer_id = %self.config.indexer_id,
                    root_space_id = %encode_id(self.config.root_space_id),
                    graph_state_version = self.config.graph_state_version,
                    "No checkpoint found; starting fresh"
                );
                Ok(None)
            }
            Err(err) => {
                error!(
                    indexer_id = %self.config.indexer_id,
                    root_space_id = %encode_id(self.config.root_space_id),
                    graph_state_version = self.config.graph_state_version,
                    reason = %err,
                    "Checkpoint load failed while persistence is enabled"
                );
                Err(err)
            }
        }
    }

    pub async fn wait_for_persistence_recovery_if_paused(&self) -> Result<(), CheckpointError> {
        loop {
            let pending = {
                let state = self.fail_open.lock().unwrap();
                if !state.paused {
                    return Ok(());
                }
                state.pending_checkpoint.clone()
            };

            let Some(checkpoint) = pending else {
                let mut state = self.fail_open.lock().unwrap();
                state.paused = false;
                state.pause_recovery_attempts = 0;
                return Ok(());
            };

            match self.try_persist_once(&checkpoint).await {
                Ok(_) => {
                    self.record_persist_success();
                    info!(
                        block_number = checkpoint.block_number,
                        cursor = %checkpoint.cursor,
                        "Checkpoint persistence recovered; resuming processing"
                    );
                    return Ok(());
                }
                Err(err) => {
                    let attempts = {
                        let mut state = self.fail_open.lock().unwrap();
                        state.pause_recovery_attempts =
                            state.pause_recovery_attempts.saturating_add(1);
                        state.pause_recovery_attempts
                    };

                    warn!(
                        indexer_id = %self.config.indexer_id,
                        root_space_id = %encode_id(self.config.root_space_id),
                        graph_state_version = self.config.graph_state_version,
                        reason = %err,
                        pause_recovery_attempt = attempts,
                        pause_recovery_max_attempts = self.config.pause_recovery_max_attempts,
                        block_number = checkpoint.block_number,
                        cursor = %checkpoint.cursor,
                        "Still paused due to checkpoint outage; retrying"
                    );

                    if attempts >= self.config.pause_recovery_max_attempts {
                        error!(
                            indexer_id = %self.config.indexer_id,
                            root_space_id = %encode_id(self.config.root_space_id),
                            graph_state_version = self.config.graph_state_version,
                            block_number = checkpoint.block_number,
                            cursor = %checkpoint.cursor,
                            pause_recovery_attempt = attempts,
                            pause_recovery_max_attempts = self.config.pause_recovery_max_attempts,
                            "Pause recovery attempts exhausted"
                        );
                        return Err(CheckpointError::Io(
                            "checkpoint pause recovery attempts exhausted".to_string(),
                        ));
                    }

                    tokio::time::sleep(Duration::from_millis(
                        self.config.checkpoint_retry_backoff_ms,
                    ))
                    .await;
                }
            }
        }
    }

    pub async fn persist_block_checkpoint(
        &self,
        block_number: u64,
        cursor: String,
        graph_state_blob: PersistedGraphState,
    ) {
        if self.config.store.is_none() {
            return;
        }

        let checkpoint = Checkpoint::new(
            self.config.indexer_id.clone(),
            cursor,
            block_number,
            graph_state_blob,
            self.config.graph_state_version,
            self.config.runtime_compatibility_marker.clone(),
            self.config.root_space_id,
        );

        match self.try_persist_with_retries(&checkpoint).await {
            Ok(()) => {
                self.record_persist_success();
            }
            Err(err) => {
                self.record_persist_failure(block_number, &checkpoint, &err);
            }
        }
    }

    fn record_persist_success(&self) {
        let mut state = self.fail_open.lock().unwrap();
        if state.consecutive_uncheckpointed_blocks > 0 {
            info!(
                recovered_after_blocks = state.consecutive_uncheckpointed_blocks,
                "Recovered from checkpoint write failures"
            );
        }
        state.consecutive_uncheckpointed_blocks = 0;
        state.pending_checkpoint = None;
        state.paused = false;
        state.pause_recovery_attempts = 0;
    }

    fn record_persist_failure(&self, block_number: u64, checkpoint: &Checkpoint, reason: &str) {
        let mut state = self.fail_open.lock().unwrap();
        state.consecutive_uncheckpointed_blocks += 1;
        let fail_open_count = state.consecutive_uncheckpointed_blocks;
        state.pending_checkpoint = Some(checkpoint.clone());

        if fail_open_count == 1 {
            warn!(
                indexer_id = %self.config.indexer_id,
                root_space_id = %encode_id(self.config.root_space_id),
                graph_state_version = self.config.graph_state_version,
                reason = reason,
                block_number,
                fail_open_bound = self.config.fail_open_bound,
                "Entering fail-open mode due to checkpoint failure"
            );
        } else {
            warn!(
                indexer_id = %self.config.indexer_id,
                root_space_id = %encode_id(self.config.root_space_id),
                graph_state_version = self.config.graph_state_version,
                reason = reason,
                block_number,
                consecutive_uncheckpointed_blocks = fail_open_count,
                fail_open_bound = self.config.fail_open_bound,
                "Checkpoint write still failing"
            );
        }

        if fail_open_count > self.config.fail_open_bound {
            state.paused = true;
            state.pause_recovery_attempts = 0;
            error!(
                indexer_id = %self.config.indexer_id,
                root_space_id = %encode_id(self.config.root_space_id),
                graph_state_version = self.config.graph_state_version,
                block_number,
                consecutive_uncheckpointed_blocks = fail_open_count,
                fail_open_bound = self.config.fail_open_bound,
                "Fail-open bound exceeded; pausing processing until checkpoint write recovers"
            );
        }
    }

    async fn try_persist_with_retries(&self, checkpoint: &Checkpoint) -> Result<(), String> {
        for attempt in 1..=self.config.checkpoint_retry_attempts {
            let persist_start = std::time::Instant::now();
            match self.try_persist_once(checkpoint).await {
                Ok(size_bytes) => {
                    let persist_latency_ms = persist_start.elapsed().as_millis() as u64;
                    info!(
                        indexer_id = %self.config.indexer_id,
                        root_space_id = %encode_id(self.config.root_space_id),
                        graph_state_version = self.config.graph_state_version,
                        block_number = checkpoint.block_number,
                        cursor = %checkpoint.cursor,
                        snapshot_size_bytes = size_bytes,
                        persist_latency_ms,
                        attempt,
                        "Checkpoint persisted"
                    );
                    return Ok(());
                }
                Err(err) => {
                    if attempt == self.config.checkpoint_retry_attempts {
                        return Err(err.to_string());
                    }

                    let backoff_ms = self
                        .config
                        .checkpoint_retry_backoff_ms
                        .saturating_mul(u64::from(attempt));
                    warn!(
                        indexer_id = %self.config.indexer_id,
                        root_space_id = %encode_id(self.config.root_space_id),
                        graph_state_version = self.config.graph_state_version,
                        reason = %err,
                        attempt,
                        retry_backoff_ms = backoff_ms,
                        "Checkpoint persist failed; retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                }
            }
        }

        Err("checkpoint write retries exhausted".to_string())
    }

    async fn try_persist_once(&self, checkpoint: &Checkpoint) -> Result<usize, CheckpointError> {
        let Some(store) = &self.config.store else {
            return Ok(0);
        };

        store.save(checkpoint).await
    }
}

#[derive(Debug, Clone)]
pub struct PostgresCheckpointStore {
    pool: PgPool,
}

impl PostgresCheckpointStore {
    pub fn new(
        database_url: &str,
        pool_config: &PostgresPoolConfig,
    ) -> Result<Self, CheckpointError> {
        info!(
            max_connections = pool_config.max_connections,
            min_connections = pool_config.min_connections,
            acquire_timeout_ms = pool_config.acquire_timeout_ms,
            idle_timeout_ms = pool_config.idle_timeout_ms,
            max_lifetime_ms = pool_config.max_lifetime_ms,
            statement_timeout_ms = pool_config.statement_timeout_ms,
            "Configuring Atlas checkpoint Postgres pool"
        );

        let statement_timeout_ms =
            i32::try_from(pool_config.statement_timeout_ms).map_err(|_| {
                CheckpointError::Incompatible(format!(
                    "ATLAS_CHECKPOINT_STATEMENT_TIMEOUT_MS out of range for Postgres: {}",
                    pool_config.statement_timeout_ms
                ))
            })?;
        let statement_timeout_query = format!("SET statement_timeout = {}", statement_timeout_ms);
        let pool = PgPoolOptions::new()
            .max_connections(pool_config.max_connections)
            .min_connections(pool_config.min_connections)
            .acquire_timeout(Duration::from_millis(pool_config.acquire_timeout_ms))
            .idle_timeout(Duration::from_millis(pool_config.idle_timeout_ms))
            .max_lifetime(Duration::from_millis(pool_config.max_lifetime_ms))
            .after_connect(move |conn, _meta| {
                let statement_timeout_query = statement_timeout_query.clone();
                Box::pin(async move {
                    sqlx::query(&statement_timeout_query).execute(conn).await?;
                    Ok(())
                })
            })
            .connect_lazy(database_url)
            .map_err(|err| CheckpointError::Io(format!("create postgres pool: {err}")))?;
        Ok(Self { pool })
    }

    pub async fn load(&self, indexer_id: &str) -> Result<Option<Checkpoint>, CheckpointError> {
        let row = sqlx::query(
            "SELECT
                schema_version,
                indexer_id,
                cursor,
                block_number,
                graph_state_blob,
                graph_state_version,
                runtime_compatibility_marker,
                root_space_id
             FROM atlas_checkpoints
             WHERE indexer_id = $1",
        )
        .bind(indexer_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| CheckpointError::Io(format!("load checkpoint: {err}")))?;

        let Some(row) = row else {
            return Ok(None);
        };

        let graph_state_blob: PersistedGraphState = row
            .try_get::<sqlx::types::Json<PersistedGraphState>, _>("graph_state_blob")
            .map_err(|err| {
                CheckpointError::Serialization(format!("decode graph_state_blob: {err}"))
            })?
            .0;

        let schema_version_i16 = row.try_get::<i16, _>("schema_version").map_err(|err| {
            CheckpointError::Serialization(format!("decode schema_version: {err}"))
        })?;
        let schema_version = u32::try_from(schema_version_i16).map_err(|_| {
            CheckpointError::Serialization(format!(
                "invalid schema_version value: {schema_version_i16}"
            ))
        })?;

        let block_number_i64 = row
            .try_get::<i64, _>("block_number")
            .map_err(|err| CheckpointError::Serialization(format!("decode block_number: {err}")))?;
        let block_number = u64::try_from(block_number_i64).map_err(|_| {
            CheckpointError::Serialization(format!(
                "invalid block_number value: {block_number_i64}"
            ))
        })?;

        let graph_state_version_i16 =
            row.try_get::<i16, _>("graph_state_version")
                .map_err(|err| {
                    CheckpointError::Serialization(format!("decode graph_state_version: {err}"))
                })?;
        let graph_state_version = u32::try_from(graph_state_version_i16).map_err(|_| {
            CheckpointError::Serialization(format!(
                "invalid graph_state_version value: {graph_state_version_i16}"
            ))
        })?;

        Ok(Some(Checkpoint {
            schema_version,
            indexer_id: row.try_get("indexer_id").map_err(|err| {
                CheckpointError::Serialization(format!("decode indexer_id: {err}"))
            })?,
            cursor: row
                .try_get("cursor")
                .map_err(|err| CheckpointError::Serialization(format!("decode cursor: {err}")))?,
            block_number,
            graph_state_blob,
            graph_state_version,
            runtime_compatibility_marker: row.try_get("runtime_compatibility_marker").map_err(
                |err| {
                    CheckpointError::Serialization(format!(
                        "decode runtime_compatibility_marker: {err}"
                    ))
                },
            )?,
            root_space_id: row.try_get("root_space_id").map_err(|err| {
                CheckpointError::Serialization(format!("decode root_space_id: {err}"))
            })?,
        }))
    }

    pub async fn save(&self, checkpoint: &Checkpoint) -> Result<usize, CheckpointError> {
        let graph_blob_json = sqlx::types::Json(checkpoint.graph_state_blob.clone());

        let db_block_number = i64::try_from(checkpoint.block_number).map_err(|_| {
            CheckpointError::Serialization(format!(
                "block_number out of range for BIGINT: {}",
                checkpoint.block_number
            ))
        })?;

        let db_graph_state_version =
            i16::try_from(checkpoint.graph_state_version).map_err(|_| {
                CheckpointError::Serialization(format!(
                    "graph_state_version out of range for SMALLINT: {}",
                    checkpoint.graph_state_version
                ))
            })?;

        let db_schema_version = i16::try_from(checkpoint.schema_version).map_err(|_| {
            CheckpointError::Serialization(format!(
                "schema_version out of range for SMALLINT: {}",
                checkpoint.schema_version
            ))
        })?;

        let row = sqlx::query(
            "INSERT INTO atlas_checkpoints (
                indexer_id,
                cursor,
                block_number,
                graph_state_blob,
                graph_state_version,
                runtime_compatibility_marker,
                root_space_id,
                schema_version,
                updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW())
            ON CONFLICT (indexer_id) DO UPDATE SET
                cursor = EXCLUDED.cursor,
                block_number = EXCLUDED.block_number,
                graph_state_blob = EXCLUDED.graph_state_blob,
                graph_state_version = EXCLUDED.graph_state_version,
                runtime_compatibility_marker = EXCLUDED.runtime_compatibility_marker,
                root_space_id = EXCLUDED.root_space_id,
                schema_version = EXCLUDED.schema_version,
                updated_at = NOW()
            RETURNING pg_column_size(graph_state_blob) AS snapshot_size_bytes",
        )
        .bind(&checkpoint.indexer_id)
        .bind(&checkpoint.cursor)
        .bind(db_block_number)
        .bind(graph_blob_json)
        .bind(db_graph_state_version)
        .bind(&checkpoint.runtime_compatibility_marker)
        .bind(&checkpoint.root_space_id)
        .bind(db_schema_version)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| CheckpointError::Io(format!("save checkpoint: {err}")))?;

        let snapshot_size_bytes_i32 =
            row.try_get::<i32, _>("snapshot_size_bytes")
                .map_err(|err| {
                    CheckpointError::Serialization(format!("decode snapshot_size_bytes: {err}"))
                })?;

        let snapshot_size_bytes = usize::try_from(snapshot_size_bytes_i32).map_err(|_| {
            CheckpointError::Serialization(format!(
                "invalid snapshot_size_bytes value: {snapshot_size_bytes_i32}"
            ))
        })?;

        Ok(snapshot_size_bytes)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub schema_version: u32,
    pub indexer_id: String,
    pub cursor: String,
    pub block_number: u64,
    pub graph_state_blob: PersistedGraphState,
    pub graph_state_version: u32,
    pub runtime_compatibility_marker: String,
    pub root_space_id: String,
}

impl Checkpoint {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        indexer_id: String,
        cursor: String,
        block_number: u64,
        graph_state_blob: PersistedGraphState,
        graph_state_version: u32,
        runtime_compatibility_marker: String,
        root_space_id: SpaceId,
    ) -> Self {
        Self {
            schema_version: CHECKPOINT_SCHEMA_VERSION,
            indexer_id,
            cursor,
            block_number,
            graph_state_blob,
            graph_state_version,
            runtime_compatibility_marker,
            root_space_id: encode_id(root_space_id),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_graph_state(
        indexer_id: String,
        cursor: String,
        block_number: u64,
        graph_state: &GraphState,
        graph_state_version: u32,
        runtime_compatibility_marker: String,
        root_space_id: SpaceId,
    ) -> Self {
        Self::new(
            indexer_id,
            cursor,
            block_number,
            PersistedGraphState::from(graph_state),
            graph_state_version,
            runtime_compatibility_marker,
            root_space_id,
        )
    }

    pub fn graph_state(&self) -> Result<GraphState, CheckpointError> {
        self.graph_state_blob.to_graph_state()
    }

    pub fn validate_compatibility(
        &self,
        expected_indexer_id: &str,
        expected_runtime_marker: &str,
        expected_root_space: SpaceId,
        expected_graph_state_version: u32,
    ) -> Result<(), CheckpointError> {
        if self.schema_version != CHECKPOINT_SCHEMA_VERSION {
            return Err(CheckpointError::Incompatible(format!(
                "schema_version {} != {}",
                self.schema_version, CHECKPOINT_SCHEMA_VERSION
            )));
        }

        if self.indexer_id != expected_indexer_id {
            return Err(CheckpointError::Incompatible(format!(
                "indexer_id {} != {}",
                self.indexer_id, expected_indexer_id
            )));
        }

        if self.runtime_compatibility_marker != expected_runtime_marker {
            return Err(CheckpointError::Incompatible(format!(
                "runtime marker {} != {}",
                self.runtime_compatibility_marker, expected_runtime_marker
            )));
        }

        let expected_root = encode_id(expected_root_space);
        if self.root_space_id != expected_root {
            return Err(CheckpointError::Incompatible(format!(
                "root_space_id {} != {}",
                self.root_space_id, expected_root
            )));
        }

        if self.graph_state_version != expected_graph_state_version {
            return Err(CheckpointError::Incompatible(format!(
                "graph_state_version {} != {}",
                self.graph_state_version, expected_graph_state_version
            )));
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedGraphState {
    pub spaces: Vec<String>,
    pub space_topics: Vec<PersistedSpaceTopic>,
    pub topic_spaces: Vec<PersistedTopicMembers>,
    pub explicit_edges: Vec<PersistedExplicitEdges>,
    pub topic_edges: Vec<PersistedTopicEdges>,
    pub topic_edge_sources: Vec<PersistedTopicMembers>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedSpaceTopic {
    pub space_id: String,
    pub topic_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTopicMembers {
    pub topic_id: String,
    pub member_space_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedExplicitEdges {
    pub source_space_id: String,
    pub edges: Vec<PersistedExplicitEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedExplicitEdge {
    pub target_space_id: String,
    pub edge_type: PersistedEdgeType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTopicEdges {
    pub source_space_id: String,
    pub topic_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PersistedEdgeType {
    Verified,
    Related,
    Topic { topic_id: String },
    Root,
    Editor,
}

impl PersistedGraphState {
    pub fn to_graph_state(&self) -> Result<GraphState, CheckpointError> {
        let mut state = GraphState::new();

        state.spaces = self
            .spaces
            .iter()
            .map(|id| decode_space_id(id))
            .collect::<Result<HashSet<_>, _>>()?;

        for entry in &self.space_topics {
            state.space_topics.insert(
                decode_space_id(&entry.space_id)?,
                decode_topic_id(&entry.topic_id)?,
            );
        }

        for entry in &self.topic_spaces {
            let topic_id = decode_topic_id(&entry.topic_id)?;
            let members = entry
                .member_space_ids
                .iter()
                .map(|id| decode_space_id(id))
                .collect::<Result<HashSet<_>, _>>()?;
            state.topic_spaces.insert(topic_id, members);
        }

        for entry in &self.explicit_edges {
            let source = decode_space_id(&entry.source_space_id)?;
            let edges = entry
                .edges
                .iter()
                .map(|edge| {
                    Ok((
                        decode_space_id(&edge.target_space_id)?,
                        edge.edge_type.to_edge_type()?,
                    ))
                })
                .collect::<Result<Vec<_>, CheckpointError>>()?;
            state.explicit_edges.insert(source, edges);
        }

        for entry in &self.topic_edges {
            let source = decode_space_id(&entry.source_space_id)?;
            let topics = entry
                .topic_ids
                .iter()
                .map(|id| decode_topic_id(id))
                .collect::<Result<HashSet<_>, _>>()?;
            state.topic_edges.insert(source, topics);
        }

        for entry in &self.topic_edge_sources {
            let topic = decode_topic_id(&entry.topic_id)?;
            let members = entry
                .member_space_ids
                .iter()
                .map(|id| decode_space_id(id))
                .collect::<Result<HashSet<_>, _>>()?;
            state.topic_edge_sources.insert(topic, members);
        }

        Ok(state)
    }
}

impl From<&GraphState> for PersistedGraphState {
    fn from(state: &GraphState) -> Self {
        let spaces = sorted_space_ids(state.spaces.iter().copied().collect());

        let mut space_topics = state
            .space_topics
            .iter()
            .map(|(space_id, topic_id)| PersistedSpaceTopic {
                space_id: encode_id(*space_id),
                topic_id: encode_id(*topic_id),
            })
            .collect::<Vec<_>>();
        space_topics.sort_by(|a, b| a.space_id.cmp(&b.space_id));

        let mut topic_spaces = state
            .topic_spaces
            .iter()
            .map(|(topic_id, members)| PersistedTopicMembers {
                topic_id: encode_id(*topic_id),
                member_space_ids: sorted_space_ids(members.iter().copied().collect()),
            })
            .collect::<Vec<_>>();
        topic_spaces.sort_by(|a, b| a.topic_id.cmp(&b.topic_id));

        let mut explicit_edges = state
            .explicit_edges
            .iter()
            .map(|(source, edges)| {
                let mut encoded_edges = edges
                    .iter()
                    .map(|(target, edge_type)| PersistedExplicitEdge {
                        target_space_id: encode_id(*target),
                        edge_type: PersistedEdgeType::from_edge_type(*edge_type),
                    })
                    .collect::<Vec<_>>();
                encoded_edges.sort_by(|a, b| {
                    a.target_space_id
                        .cmp(&b.target_space_id)
                        .then_with(|| a.edge_type.rank().cmp(&b.edge_type.rank()))
                });

                PersistedExplicitEdges {
                    source_space_id: encode_id(*source),
                    edges: encoded_edges,
                }
            })
            .collect::<Vec<_>>();
        explicit_edges.sort_by(|a, b| a.source_space_id.cmp(&b.source_space_id));

        let mut topic_edges = state
            .topic_edges
            .iter()
            .map(|(source, topics)| {
                let mut topic_ids = topics.iter().map(|id| encode_id(*id)).collect::<Vec<_>>();
                topic_ids.sort();
                PersistedTopicEdges {
                    source_space_id: encode_id(*source),
                    topic_ids,
                }
            })
            .collect::<Vec<_>>();
        topic_edges.sort_by(|a, b| a.source_space_id.cmp(&b.source_space_id));

        let mut topic_edge_sources = state
            .topic_edge_sources
            .iter()
            .map(|(topic_id, members)| PersistedTopicMembers {
                topic_id: encode_id(*topic_id),
                member_space_ids: sorted_space_ids(members.iter().copied().collect()),
            })
            .collect::<Vec<_>>();
        topic_edge_sources.sort_by(|a, b| a.topic_id.cmp(&b.topic_id));

        Self {
            spaces,
            space_topics,
            topic_spaces,
            explicit_edges,
            topic_edges,
            topic_edge_sources,
        }
    }
}

impl PersistedEdgeType {
    fn from_edge_type(edge_type: EdgeType) -> Self {
        match edge_type {
            EdgeType::Root => Self::Root,
            EdgeType::Verified => Self::Verified,
            EdgeType::Related => Self::Related,
            EdgeType::Topic { topic_id } => Self::Topic {
                topic_id: encode_id(topic_id),
            },
            EdgeType::Editor => Self::Editor,
        }
    }

    fn to_edge_type(&self) -> Result<EdgeType, CheckpointError> {
        match self {
            PersistedEdgeType::Root => Ok(EdgeType::Root),
            PersistedEdgeType::Verified => Ok(EdgeType::Verified),
            PersistedEdgeType::Related => Ok(EdgeType::Related),
            PersistedEdgeType::Topic { topic_id } => Ok(EdgeType::Topic {
                topic_id: decode_topic_id(topic_id)?,
            }),
            PersistedEdgeType::Editor => Ok(EdgeType::Editor),
        }
    }

    fn rank(&self) -> u8 {
        match self {
            PersistedEdgeType::Root => 0,
            PersistedEdgeType::Verified => 1,
            PersistedEdgeType::Related => 2,
            PersistedEdgeType::Topic { .. } => 3,
            PersistedEdgeType::Editor => 4,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    #[error("I/O error: {0}")]
    Io(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Checkpoint incompatible: {0}")]
    Incompatible(String),
}

fn sorted_space_ids(mut ids: Vec<SpaceId>) -> Vec<String> {
    ids.sort();
    ids.into_iter().map(encode_id).collect()
}

fn encode_id<T: AsRef<[u8]>>(id: T) -> String {
    hex::encode(id)
}

fn decode_space_id(value: &str) -> Result<SpaceId, CheckpointError> {
    decode_fixed_id::<16>(value, "space_id")
}

fn decode_topic_id(value: &str) -> Result<TopicId, CheckpointError> {
    decode_fixed_id::<16>(value, "topic_id")
}

fn decode_fixed_id<const N: usize>(value: &str, field: &str) -> Result<[u8; N], CheckpointError> {
    let bytes = hex::decode(value)
        .map_err(|err| CheckpointError::Serialization(format!("invalid {field} hex: {err}")))?;
    if bytes.len() != N {
        return Err(CheckpointError::Serialization(format!(
            "invalid {field} length {}, expected {N}",
            bytes.len()
        )));
    }

    let mut out = [0u8; N];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn parse_env_bool(name: &str, default: bool) -> bool {
    std::env::var(name)
        .map(|value| value.eq_ignore_ascii_case("true") || value == "1")
        .unwrap_or(default)
}

fn parse_clamped_env_u32(name: &str, default: u32, min: u32, max: u32, label: &str) -> u32 {
    let raw = std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(default);
    let normalized = raw.clamp(min, max);
    if raw != normalized {
        warn!(
            env_var = name,
            config_label = label,
            provided = raw,
            normalized,
            min,
            max,
            "Normalized numeric config from environment"
        );
    }
    normalized
}

fn parse_clamped_env_u64(name: &str, default: u64, min: u64, max: u64, label: &str) -> u64 {
    let raw = std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default);
    let normalized = raw.clamp(min, max);
    if raw != normalized {
        warn!(
            env_var = name,
            config_label = label,
            provided = raw,
            normalized,
            min,
            max,
            "Normalized numeric config from environment"
        );
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    fn make_space_id(n: u8) -> SpaceId {
        let mut id = [0u8; 16];
        id[15] = n;
        id
    }

    fn make_topic_id(n: u8) -> TopicId {
        let mut id = [0u8; 16];
        id[15] = n;
        id
    }

    fn test_config(fail_open_bound: u64) -> CheckpointConfig {
        CheckpointConfig {
            root_space_id: make_space_id(1),
            graph_state_version: 1,
            indexer_id: "idx-test".to_string(),
            runtime_compatibility_marker: "atlas-v2".to_string(),
            fail_open_bound,
            checkpoint_retry_attempts: 1,
            checkpoint_retry_backoff_ms: 50,
            pause_recovery_max_attempts: 5,
            allow_fresh_start_on_invalid_checkpoint: true,
            store: None,
        }
    }

    fn checkpoint_for(block_number: u64) -> Checkpoint {
        Checkpoint::from_graph_state(
            "idx-test".to_string(),
            format!("cursor-{block_number}"),
            block_number,
            &GraphState::new(),
            1,
            "atlas-v2".to_string(),
            make_space_id(1),
        )
    }

    fn fail_open_state(manager: &CheckpointManager) -> MutexGuard<'_, FailOpenState> {
        manager.fail_open.lock().unwrap()
    }

    #[test]
    fn round_trips_non_empty_graph_state() {
        let mut graph_state = GraphState::new();
        let a = make_space_id(1);
        let b = make_space_id(2);
        let topic = make_topic_id(9);

        graph_state.spaces.insert(a);
        graph_state.spaces.insert(b);
        graph_state.space_topics.insert(a, topic);
        graph_state.space_topics.insert(b, topic);
        graph_state
            .topic_spaces
            .insert(topic, [a, b].into_iter().collect());
        graph_state
            .explicit_edges
            .insert(a, vec![(b, EdgeType::Verified)]);
        graph_state
            .topic_edges
            .insert(a, [topic].into_iter().collect());
        graph_state
            .topic_edge_sources
            .insert(topic, [a].into_iter().collect());

        let persisted = PersistedGraphState::from(&graph_state);
        let restored = persisted.to_graph_state().unwrap();

        assert_eq!(restored.space_count(), graph_state.space_count());
        assert_eq!(
            restored.explicit_edge_count(),
            graph_state.explicit_edge_count()
        );
        assert_eq!(restored.topic_edge_count(), graph_state.topic_edge_count());
        assert!(restored.contains_space(&a));
        assert!(restored.contains_space(&b));
    }

    #[test]
    fn validates_compatibility() {
        let checkpoint = Checkpoint::from_graph_state(
            "idx-test".to_string(),
            "cursor-10".to_string(),
            10,
            &GraphState::new(),
            1,
            "atlas-v2".to_string(),
            make_space_id(1),
        );

        checkpoint
            .validate_compatibility("idx-test", "atlas-v2", make_space_id(1), 1)
            .unwrap();

        let err = checkpoint
            .validate_compatibility("idx-other", "atlas-v2", make_space_id(1), 1)
            .unwrap_err();
        assert!(matches!(err, CheckpointError::Incompatible(_)));
    }

    #[test]
    fn fail_open_pauses_after_bound_is_exceeded() {
        let manager = CheckpointManager::new(test_config(2));
        let cp1 = checkpoint_for(1);
        let cp2 = checkpoint_for(2);
        let cp3 = checkpoint_for(3);

        manager.record_persist_failure(1, &cp1, "write failed");
        {
            let state = fail_open_state(&manager);
            assert_eq!(state.consecutive_uncheckpointed_blocks, 1);
            assert!(!state.paused);
            assert!(state.pending_checkpoint.is_some());
        }

        manager.record_persist_failure(2, &cp2, "write failed");
        {
            let state = fail_open_state(&manager);
            assert_eq!(state.consecutive_uncheckpointed_blocks, 2);
            assert!(!state.paused);
        }

        manager.record_persist_failure(3, &cp3, "write failed");
        {
            let state = fail_open_state(&manager);
            assert_eq!(state.consecutive_uncheckpointed_blocks, 3);
            assert!(state.paused);
            assert_eq!(state.pending_checkpoint.as_ref().unwrap().block_number, 3);
        }
    }

    #[test]
    fn persist_success_resets_fail_open_state() {
        let manager = CheckpointManager::new(test_config(2));
        let cp1 = checkpoint_for(1);

        manager.record_persist_failure(1, &cp1, "write failed");
        manager.record_persist_success();

        let state = fail_open_state(&manager);
        assert_eq!(state.consecutive_uncheckpointed_blocks, 0);
        assert!(!state.paused);
        assert!(state.pending_checkpoint.is_none());
    }
}
