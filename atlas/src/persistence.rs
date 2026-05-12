use std::collections::HashSet;
use std::time::Duration;

use crate::events::{SpaceId, TopicId};
use crate::graph::{EdgeType, GraphState};
use hermes_instrumentation::{error, info, warn};
use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgPoolOptions, PgPool, Row};

/// Persisted shape of atlas's emission contract: every (non-root) canonical
/// node we last told consumers about, as a `(space_id, distance, parent)`
/// triple.
///
/// This is the part of the checkpoint that must survive `GraphState` /
/// `EdgeType` schema bumps. It is encoded as a length-prefixed flat binary
/// blob with no field names, no enum tags, no edge-type identifier — only
/// consumer-observable primitives — so a future change to internal graph
/// types cannot break baseline restore.
///
/// On startup, `DiffTracker::from_baseline(...)` uses this to prime the
/// emission state so the first `track()` call after a rules change produces
/// the correct `REMOVED` events for orphaned spaces and `MOVED` events for
/// repositioned ones. See GEO-645 for the failure mode this guards against.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PersistedEmissionBaseline {
    /// Sorted by `space_id`, unique by `space_id` (the closest-to-root entry
    /// wins on duplicates, matching `DiffTracker`'s collapse semantics).
    nodes: Vec<BaselineNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BaselineNode {
    pub space_id: SpaceId,
    pub distance: u32,
    pub parent: SpaceId,
}

// Magic prefix isolates baseline blobs from any other bytea we might store on
// the row in the future. The trailing `\x01` is the format version: bumping it
// is the contract for "the on-disk shape of this blob changed in a
// non-backwards-compatible way", and old atlas builds must reject anything
// they don't recognise rather than silently misread.
const BASELINE_MAGIC: &[u8; 8] = b"ATLBL\x00\x00\x01";
const BASELINE_FORMAT_VERSION: u8 = 1;
// magic (8) + version (1) + node count u32 LE (4)
const BASELINE_HEADER_LEN: usize = 13;
// space_id (16) + distance u32 LE (4) + parent space_id (16)
const BASELINE_NODE_LEN: usize = 36;

impl PersistedEmissionBaseline {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn nodes(&self) -> &[BaselineNode] {
        &self.nodes
    }

    /// Build a baseline from an arbitrary iterator of node entries. The
    /// constructor sorts by `space_id` and deduplicates, picking the entry
    /// with the smallest `distance` on collisions — same "closest to root
    /// wins" rule `DiffTracker` applies to in-memory positions.
    pub fn from_nodes<I: IntoIterator<Item = BaselineNode>>(nodes: I) -> Self {
        let mut nodes: Vec<BaselineNode> = nodes.into_iter().collect();
        nodes.sort_unstable_by(|a, b| {
            a.space_id
                .cmp(&b.space_id)
                .then_with(|| a.distance.cmp(&b.distance))
        });
        nodes.dedup_by_key(|n| n.space_id);
        Self { nodes }
    }

    /// Snapshot a diff tracker's current emission state as a persistable
    /// baseline. Returns `None` if the tracker has nothing to persist (no
    /// canonical compute has ever produced output and no prior baseline was
    /// loaded), in which case the caller should not write a baseline column —
    /// writing an empty baseline would let a fresh-start atlas advertise
    /// "consumers have nothing canonical" and force a spurious wipe on the
    /// next restart.
    pub fn from_diff_tracker(tracker: &crate::graph::DiffTracker) -> Option<Self> {
        let iter = tracker.iter_emission_state()?;
        let nodes: Vec<BaselineNode> = iter
            .map(|(space_id, distance, parent)| BaselineNode {
                space_id,
                distance,
                parent,
            })
            .collect();
        // The tracker's emission state is already sorted+deduped by SpaceId,
        // so skip the normalisation cost.
        Some(Self { nodes })
    }

    /// Encode as a flat little-endian binary blob.
    ///
    /// Layout (header followed by `count` fixed-size node entries):
    ///   bytes  0..8  : magic `ATLBL\0\0\x01`
    ///   byte   8     : format version (currently 1)
    ///   bytes  9..13 : node count, u32 LE
    ///   for each node (36 bytes):
    ///       16 bytes : space_id
    ///        4 bytes : distance, u32 LE
    ///       16 bytes : parent space_id
    pub fn encode(&self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(BASELINE_HEADER_LEN + self.nodes.len() * BASELINE_NODE_LEN);
        out.extend_from_slice(BASELINE_MAGIC);
        out.push(BASELINE_FORMAT_VERSION);
        let count: u32 = self
            .nodes
            .len()
            .try_into()
            .expect("baseline node count fits in u32");
        out.extend_from_slice(&count.to_le_bytes());
        for node in &self.nodes {
            out.extend_from_slice(&node.space_id);
            out.extend_from_slice(&node.distance.to_le_bytes());
            out.extend_from_slice(&node.parent);
        }
        out
    }

    /// Decode a baseline blob. Rejects unknown magic / version / truncated
    /// payloads explicitly so a corrupted column can't be silently misread.
    pub fn decode(bytes: &[u8]) -> Result<Self, CheckpointError> {
        if bytes.len() < BASELINE_HEADER_LEN {
            return Err(CheckpointError::Serialization(format!(
                "baseline blob too short: {} bytes, expected at least {}",
                bytes.len(),
                BASELINE_HEADER_LEN
            )));
        }
        if &bytes[0..8] != BASELINE_MAGIC {
            return Err(CheckpointError::Serialization(format!(
                "baseline blob magic mismatch: {:02x?}",
                &bytes[0..8]
            )));
        }
        let version = bytes[8];
        if version != BASELINE_FORMAT_VERSION {
            return Err(CheckpointError::Serialization(format!(
                "unsupported baseline format version: {version}"
            )));
        }
        let mut count_bytes = [0u8; 4];
        count_bytes.copy_from_slice(&bytes[9..13]);
        let count = u32::from_le_bytes(count_bytes) as usize;

        // Use checked arithmetic so a corrupted `count` header on a 32-bit
        // target (or a pathological value on 64-bit) cannot wrap or trigger
        // an OOM-sized allocation attempt before we get to the
        // length-vs-payload check below. On overflow we surface
        // `CheckpointError::Serialization`, which the startup path treats as
        // a recoverable "no baseline" condition rather than a panic.
        let expected_payload =
            count
                .checked_mul(BASELINE_NODE_LEN)
                .ok_or_else(|| {
                    CheckpointError::Serialization(format!(
                        "baseline blob header count {count} overflows usize when multiplied by node size {BASELINE_NODE_LEN}"
                    ))
                })?;
        let expected_len = BASELINE_HEADER_LEN.checked_add(expected_payload).ok_or_else(|| {
            CheckpointError::Serialization(format!(
                "baseline blob header count {count} overflows usize when added to header size {BASELINE_HEADER_LEN}"
            ))
        })?;
        if bytes.len() != expected_len {
            return Err(CheckpointError::Serialization(format!(
                "baseline blob length {} does not match header count {} (expected {})",
                bytes.len(),
                count,
                expected_len
            )));
        }

        let mut nodes = Vec::with_capacity(count);
        let mut offset = BASELINE_HEADER_LEN;
        for _ in 0..count {
            let mut space_id = [0u8; 16];
            space_id.copy_from_slice(&bytes[offset..offset + 16]);
            let mut dist_bytes = [0u8; 4];
            dist_bytes.copy_from_slice(&bytes[offset + 16..offset + 20]);
            let distance = u32::from_le_bytes(dist_bytes);
            let mut parent = [0u8; 16];
            parent.copy_from_slice(&bytes[offset + 20..offset + 36]);
            nodes.push(BaselineNode {
                space_id,
                distance,
                parent,
            });
            offset += BASELINE_NODE_LEN;
        }

        Ok(Self { nodes })
    }
}

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
    restored_block_number: Option<u64>,
}

/// Result of `CheckpointManager::restore_checkpoint_on_startup`.
///
/// `graph_state` and `baseline` decode independently. In particular, a
/// missing/invalid `graph_state` (marker mismatch, unknown enum variant,
/// fresh-start) must not suppress a valid `baseline` — that's how a
/// rules-change deploy gets the contract from the previous deploy.
#[derive(Debug, Default)]
pub struct RestoredCheckpoint {
    pub graph_state: Option<GraphState>,
    pub baseline: Option<PersistedEmissionBaseline>,
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
            restored_block_number: None,
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

    /// Block number associated with the restored checkpoint cursor, when
    /// one was loaded. Used by the force-write-on-startup path to write a
    /// baseline using the existing checkpoint coordinates rather than
    /// inventing a synthetic one.
    pub fn restored_block_number(&self) -> Option<u64> {
        self.restored_block_number
    }

    /// Restore graph state and the persisted emission baseline.
    ///
    /// The two are independently decoded so that a `GraphState` schema bump
    /// (runtime marker mismatch, unknown enum variant in the JSON blob) does
    /// *not* prevent the baseline from being restored — that's the whole
    /// point of the schema-stable baseline shape. See GEO-645.
    ///
    /// Returns `(graph_state, baseline)`. Either side can be `None`:
    /// - `graph_state` is `None` on fresh-start (no checkpoint, marker
    ///   mismatch with `ATLAS_CHECKPOINT_ALLOW_FRESH_START`, or unreadable
    ///   graph blob with the same env flag set).
    /// - `baseline` is `None` for a brand-new indexer that has never written
    ///   one, or when the baseline column is `NULL` on the existing row
    ///   (e.g. the first time this code runs against a pre-existing
    ///   checkpoint). Callers should force-write a baseline once they have
    ///   one to persist.
    pub async fn restore_checkpoint_on_startup(
        &mut self,
    ) -> Result<RestoredCheckpoint, CheckpointError> {
        let Some(store) = &self.config.store else {
            info!("Checkpoint persistence disabled (no ATLAS_CHECKPOINT_DATABASE_URL)");
            return Ok(RestoredCheckpoint::default());
        };

        match store.load(&self.config.indexer_id).await {
            Ok(Some(checkpoint)) => {
                // Decode the baseline first and unconditionally — even when
                // the graph_state path bails out, the caller still needs the
                // baseline so DiffTracker can be primed.
                let baseline = match checkpoint.emission_baseline() {
                    Ok(Some(baseline)) => {
                        info!(
                            indexer_id = %self.config.indexer_id,
                            root_space_id = %encode_id(self.config.root_space_id),
                            baseline_node_count = baseline.len(),
                            block_number = checkpoint.block_number,
                            cursor = %checkpoint.cursor,
                            "Emission baseline restored"
                        );
                        Some(baseline)
                    }
                    Ok(None) => {
                        info!(
                            indexer_id = %self.config.indexer_id,
                            block_number = checkpoint.block_number,
                            "No emission baseline on existing checkpoint; will force-write on first canonical compute"
                        );
                        None
                    }
                    Err(err) => {
                        // A bad baseline blob is a serious operational
                        // signal — surface it, but don't block startup.
                        // Treating it as "no baseline" triggers the
                        // force-write path on next compute, which will
                        // replace the bad blob with a known-good one.
                        error!(
                            indexer_id = %self.config.indexer_id,
                            reason = %err,
                            "Emission baseline failed to decode; proceeding without prime (will force-write)"
                        );
                        None
                    }
                };

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
                            baseline_node_count = baseline.as_ref().map(|b| b.len()).unwrap_or(0),
                            "Checkpoint rejected; ATLAS_CHECKPOINT_ALLOW_FRESH_START enabled, starting fresh (baseline retained if present)"
                        );
                        return Ok(RestoredCheckpoint {
                            graph_state: None,
                            baseline,
                        });
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
                                baseline_node_count = baseline.as_ref().map(|b| b.len()).unwrap_or(0),
                                "Checkpoint graph state unreadable; ATLAS_CHECKPOINT_ALLOW_FRESH_START enabled, starting fresh (baseline retained if present)"
                            );
                            return Ok(RestoredCheckpoint {
                                graph_state: None,
                                baseline,
                            });
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
                self.restored_block_number = Some(checkpoint.block_number);
                info!(
                    indexer_id = %self.config.indexer_id,
                    root_space_id = %encode_id(self.config.root_space_id),
                    graph_state_version = self.config.graph_state_version,
                    block_number = checkpoint.block_number,
                    cursor = %checkpoint.cursor,
                    "Checkpoint restored"
                );
                Ok(RestoredCheckpoint {
                    graph_state: Some(graph_state),
                    baseline,
                })
            }
            Ok(None) => {
                info!(
                    indexer_id = %self.config.indexer_id,
                    root_space_id = %encode_id(self.config.root_space_id),
                    graph_state_version = self.config.graph_state_version,
                    "No checkpoint found; starting fresh"
                );
                Ok(RestoredCheckpoint::default())
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

    /// Persist a per-block checkpoint. Returns `Ok(())` on a confirmed
    /// successful DB write, `Err(_)` if every retry failed.
    ///
    /// Fail-open semantics are unchanged: failures still go through
    /// `record_persist_failure` (which feeds the consecutive-failure counter,
    /// pause logic, and pending checkpoint). Callers that don't care about
    /// the result can ignore the returned `Result` — this is the case on
    /// the per-block hot path, where the next block's persist would anyway
    /// supersede this one.
    ///
    /// The signal exists for callers that need to know whether a baseline
    /// actually reached disk (e.g. the GEO-645 startup force-write and its
    /// quiet-stream retry) before clearing pending flags or emitting a
    /// "persisted for the first time" log.
    pub async fn persist_block_checkpoint(
        &self,
        block_number: u64,
        cursor: String,
        graph_state_blob: PersistedGraphState,
        emission_baseline: Option<&PersistedEmissionBaseline>,
    ) -> Result<(), CheckpointError> {
        if self.config.store.is_none() {
            return Ok(());
        }

        let checkpoint = Checkpoint::new(
            self.config.indexer_id.clone(),
            cursor,
            block_number,
            graph_state_blob,
            emission_baseline,
            self.config.graph_state_version,
            self.config.runtime_compatibility_marker.clone(),
            self.config.root_space_id,
        );

        match self.try_persist_with_retries(&checkpoint).await {
            Ok(()) => {
                self.record_persist_success();
                Ok(())
            }
            Err(err) => {
                self.record_persist_failure(block_number, &checkpoint, &err);
                Err(CheckpointError::Io(err))
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
                root_space_id,
                emission_baseline_blob
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

        // Baseline column is nullable: pre-existing rows from before this
        // migration shipped will return None here, which triggers the
        // force-write-on-startup path on the caller. Decoding is lazy
        // (`Checkpoint::emission_baseline()`) for the same reason graph
        // state decode is lazy — keep raw bytes here so a bad blob doesn't
        // prevent the rest of the checkpoint from loading.
        let emission_baseline_blob: Option<Vec<u8>> = row
            .try_get::<Option<Vec<u8>>, _>("emission_baseline_blob")
            .map_err(|err| {
                CheckpointError::Serialization(format!("decode emission_baseline_blob: {err}"))
            })?;

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
            emission_baseline_blob,
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

        // The baseline column is written in the *same* INSERT as the graph
        // state, so they cannot diverge — they share the row's transaction.
        // Passing `None` (when the caller has no baseline yet) preserves any
        // existing value, since COALESCE keeps the prior row's bytes.
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
                emission_baseline_blob,
                updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
            ON CONFLICT (indexer_id) DO UPDATE SET
                cursor = EXCLUDED.cursor,
                block_number = EXCLUDED.block_number,
                graph_state_blob = EXCLUDED.graph_state_blob,
                graph_state_version = EXCLUDED.graph_state_version,
                runtime_compatibility_marker = EXCLUDED.runtime_compatibility_marker,
                root_space_id = EXCLUDED.root_space_id,
                schema_version = EXCLUDED.schema_version,
                emission_baseline_blob = COALESCE(EXCLUDED.emission_baseline_blob, atlas_checkpoints.emission_baseline_blob),
                updated_at = NOW()
            RETURNING pg_column_size(graph_state_blob) AS snapshot_size_bytes,
                      pg_column_size(emission_baseline_blob) AS baseline_size_bytes",
        )
        .bind(&checkpoint.indexer_id)
        .bind(&checkpoint.cursor)
        .bind(db_block_number)
        .bind(graph_blob_json)
        .bind(db_graph_state_version)
        .bind(&checkpoint.runtime_compatibility_marker)
        .bind(&checkpoint.root_space_id)
        .bind(db_schema_version)
        .bind(checkpoint.emission_baseline_blob.as_deref())
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
    // Raw bytes of the persisted emission baseline (see
    // `PersistedEmissionBaseline`). Stored as a separate column so a
    // `graph_state_blob` decode failure cannot prevent the baseline from
    // loading. `None` for indexers that haven't written one yet (either
    // pre-migration rows or a brand-new fresh-start).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emission_baseline_blob: Option<Vec<u8>>,
}

impl Checkpoint {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        indexer_id: String,
        cursor: String,
        block_number: u64,
        graph_state_blob: PersistedGraphState,
        emission_baseline: Option<&PersistedEmissionBaseline>,
        graph_state_version: u32,
        runtime_compatibility_marker: String,
        root_space_id: SpaceId,
    ) -> Self {
        let emission_baseline_blob = emission_baseline.map(|b| b.encode());
        Self {
            schema_version: CHECKPOINT_SCHEMA_VERSION,
            indexer_id,
            cursor,
            block_number,
            graph_state_blob,
            graph_state_version,
            runtime_compatibility_marker,
            root_space_id: encode_id(root_space_id),
            emission_baseline_blob,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_graph_state(
        indexer_id: String,
        cursor: String,
        block_number: u64,
        graph_state: &GraphState,
        emission_baseline: Option<&PersistedEmissionBaseline>,
        graph_state_version: u32,
        runtime_compatibility_marker: String,
        root_space_id: SpaceId,
    ) -> Self {
        Self::new(
            indexer_id,
            cursor,
            block_number,
            PersistedGraphState::from(graph_state),
            emission_baseline,
            graph_state_version,
            runtime_compatibility_marker,
            root_space_id,
        )
    }

    pub fn graph_state(&self) -> Result<GraphState, CheckpointError> {
        self.graph_state_blob.to_graph_state()
    }

    /// Lazily decode the persisted emission baseline. Returns `Ok(None)` when
    /// the column is `NULL` (no baseline persisted yet). Decode failures are
    /// returned as `Err` so the caller can choose whether to abort or treat
    /// the row as "no baseline" (the startup path takes the latter approach
    /// and force-writes a fresh one).
    pub fn emission_baseline(&self) -> Result<Option<PersistedEmissionBaseline>, CheckpointError> {
        let Some(blob) = self.emission_baseline_blob.as_deref() else {
            return Ok(None);
        };
        PersistedEmissionBaseline::decode(blob).map(Some)
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
        }
    }

    fn rank(&self) -> u8 {
        match self {
            PersistedEdgeType::Root => 0,
            PersistedEdgeType::Verified => 1,
            PersistedEdgeType::Related => 2,
            PersistedEdgeType::Topic { .. } => 3,
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
            None,
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
            None,
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

    // ------------------------------------------------------------------
    // PersistedEmissionBaseline (GEO-645)
    // ------------------------------------------------------------------

    fn baseline_node(space: u8, distance: u32, parent: u8) -> BaselineNode {
        BaselineNode {
            space_id: make_space_id(space),
            distance,
            parent: make_space_id(parent),
        }
    }

    #[test]
    fn baseline_round_trips_through_encode_decode() {
        let baseline = PersistedEmissionBaseline::from_nodes([
            baseline_node(0x0A, 1, 0x01),
            baseline_node(0x0B, 2, 0x0A),
            baseline_node(0x0C, 1, 0x01),
        ]);
        let bytes = baseline.encode();
        let decoded = PersistedEmissionBaseline::decode(&bytes).unwrap();
        assert_eq!(decoded, baseline);
        // Ordering invariant: sorted by space_id.
        let ids: Vec<u8> = decoded.nodes().iter().map(|n| n.space_id[15]).collect();
        assert_eq!(ids, vec![0x0A, 0x0B, 0x0C]);
    }

    #[test]
    fn baseline_empty_encoding_is_just_header() {
        let bytes = PersistedEmissionBaseline::empty().encode();
        // 8 magic + 1 version + 4 count
        assert_eq!(bytes.len(), 13);
        let decoded = PersistedEmissionBaseline::decode(&bytes).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn baseline_byte_pinned_layout() {
        // Pin the on-disk layout against a hand-built expected byte sequence.
        // If this test fails, a future change has broken the schema-stable
        // promise — bump BASELINE_FORMAT_VERSION and write a compat reader
        // before merging.
        let baseline =
            PersistedEmissionBaseline::from_nodes([baseline_node(0x0A, 0x01020304, 0x0B)]);
        let bytes = baseline.encode();

        let mut expected = Vec::new();
        // magic: ATLBL\0\0\x01
        expected.extend_from_slice(b"ATLBL\x00\x00\x01");
        // version byte
        expected.push(1);
        // count = 1, LE
        expected.extend_from_slice(&1u32.to_le_bytes());
        // node: space_id (0x0A in last byte), distance LE, parent (0x0B in last byte)
        expected.extend_from_slice(&make_space_id(0x0A));
        expected.extend_from_slice(&0x01020304u32.to_le_bytes());
        expected.extend_from_slice(&make_space_id(0x0B));

        assert_eq!(bytes, expected);
    }

    #[test]
    fn baseline_rejects_bad_magic() {
        let mut bytes = PersistedEmissionBaseline::empty().encode();
        bytes[0] = b'X';
        let err = PersistedEmissionBaseline::decode(&bytes).unwrap_err();
        assert!(
            matches!(err, CheckpointError::Serialization(ref msg) if msg.contains("magic mismatch")),
            "expected magic mismatch, got: {err:?}"
        );
    }

    #[test]
    fn baseline_rejects_unknown_version() {
        let mut bytes = PersistedEmissionBaseline::empty().encode();
        bytes[8] = 2; // pretend a future version wrote this
        let err = PersistedEmissionBaseline::decode(&bytes).unwrap_err();
        assert!(
            matches!(err, CheckpointError::Serialization(ref msg) if msg.contains("unsupported baseline format version")),
            "expected version error, got: {err:?}"
        );
    }

    #[test]
    fn baseline_rejects_truncated_payload() {
        let baseline = PersistedEmissionBaseline::from_nodes([baseline_node(0x0A, 1, 0x01)]);
        let mut bytes = baseline.encode();
        bytes.pop(); // drop one byte from the node payload
        let err = PersistedEmissionBaseline::decode(&bytes).unwrap_err();
        assert!(
            matches!(err, CheckpointError::Serialization(ref msg) if msg.contains("does not match header count")),
            "expected length-mismatch error, got: {err:?}"
        );
    }

    #[test]
    fn baseline_rejects_count_that_would_overflow_or_overallocate() {
        // A corrupted header with `count = u32::MAX` would, on a 32-bit
        // target, wrap `count * BASELINE_NODE_LEN` past `usize::MAX`, and on
        // a 64-bit target would request a ~140 GB allocation. The decode
        // path must reject this with `CheckpointError::Serialization`
        // rather than panicking or hitting the allocator. We hand-build a
        // header with the pathological count and a truncated body so the
        // arithmetic / length-mismatch check fires before any slice indexing
        // into the (absent) node payload.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(BASELINE_MAGIC);
        bytes.push(BASELINE_FORMAT_VERSION);
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        // Intentionally no node payload follows.
        let err = PersistedEmissionBaseline::decode(&bytes).unwrap_err();
        // On 32-bit `usize`, this hits the `checked_mul` overflow branch;
        // on 64-bit, it falls through to the length-vs-payload mismatch.
        // Either way the contract is the same: surface as `Serialization`
        // (never panic / OOM) so the startup path can recover.
        assert!(
            matches!(err, CheckpointError::Serialization(_)),
            "expected Serialization error, got: {err:?}"
        );
    }

    #[test]
    fn baseline_dedupe_keeps_smallest_distance() {
        // Same space_id appearing twice at different distances: the smaller
        // distance must win (closest-to-root rule, matches DiffTracker).
        let baseline = PersistedEmissionBaseline::from_nodes([
            baseline_node(0x0B, 5, 0x0A),
            baseline_node(0x0B, 1, 0x01),
        ]);
        assert_eq!(baseline.len(), 1);
        let node = &baseline.nodes()[0];
        assert_eq!(node.distance, 1);
        assert_eq!(node.parent, make_space_id(0x01));
    }

    #[test]
    fn checkpoint_carries_baseline_through_struct_constructor() {
        let baseline = PersistedEmissionBaseline::from_nodes([baseline_node(0x0A, 1, 0x01)]);
        let cp = Checkpoint::from_graph_state(
            "idx-test".to_string(),
            "cursor-1".to_string(),
            1,
            &GraphState::new(),
            Some(&baseline),
            1,
            "atlas-v2".to_string(),
            make_space_id(1),
        );
        let restored = cp
            .emission_baseline()
            .expect("baseline decodes")
            .expect("baseline present");
        assert_eq!(restored, baseline);
    }

    #[test]
    fn checkpoint_omits_baseline_when_none() {
        let cp = Checkpoint::from_graph_state(
            "idx-test".to_string(),
            "cursor-1".to_string(),
            1,
            &GraphState::new(),
            None,
            1,
            "atlas-v2".to_string(),
            make_space_id(1),
        );
        assert!(cp.emission_baseline_blob.is_none());
        assert!(cp.emission_baseline().unwrap().is_none());
    }

    #[test]
    fn baseline_decode_failure_surfaces_serialization_error() {
        // A `Checkpoint` with a corrupted baseline blob must report the
        // failure as `Serialization` so the startup path can route it
        // through the "treat as no baseline, force-write" fallback rather
        // than aborting.
        let cp = Checkpoint {
            schema_version: CHECKPOINT_SCHEMA_VERSION,
            indexer_id: "idx-test".to_string(),
            cursor: "cursor-1".to_string(),
            block_number: 1,
            graph_state_blob: PersistedGraphState::from(&GraphState::new()),
            graph_state_version: 1,
            runtime_compatibility_marker: "atlas-v2".to_string(),
            root_space_id: encode_id(make_space_id(1)),
            emission_baseline_blob: Some(b"not a baseline".to_vec()),
        };
        let err = cp.emission_baseline().unwrap_err();
        assert!(matches!(err, CheckpointError::Serialization(_)));
    }
}
