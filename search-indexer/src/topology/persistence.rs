//! JSON file persistence for canonical graph state.
//!
//! Saves/loads graph state using write-then-rename for atomicity.
//! Path configured via `TOPOLOGY_STATE_PATH` env var.

use std::io::Write;
use std::path::Path;

use hermes_instrumentation::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::state::CanonicalGraphState;

/// Errors that can occur during topology state persistence.
#[derive(Error, Debug)]
pub enum PersistenceError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("invalid state data: {0}")]
    InvalidData(String),
}

/// Default path for the topology state file.
pub const DEFAULT_TOPOLOGY_STATE_PATH: &str = "/data/topology_state.json";

/// Persisted graph state format.
#[derive(Serialize, Deserialize)]
struct PersistedState {
    version: u32,
    root_id: Option<String>,
    nodes: Vec<PersistedNode>,
}

#[derive(Serialize, Deserialize)]
struct PersistedNode {
    space_id: String,
    parent_id: String,
    distance: u32,
}

fn bytes_to_hex(bytes: &[u8; 16]) -> String {
    hex::encode(bytes)
}

fn hex_to_bytes(s: &str) -> Result<[u8; 16], PersistenceError> {
    let decoded =
        hex::decode(s).map_err(|e| PersistenceError::InvalidData(format!("invalid hex: {}", e)))?;
    if decoded.len() != 16 {
        return Err(PersistenceError::InvalidData(format!(
            "expected 16 bytes, got {}",
            decoded.len()
        )));
    }
    let mut arr = [0u8; 16];
    arr.copy_from_slice(&decoded);
    Ok(arr)
}

/// Save graph state to a JSON file using write-then-rename for atomicity.
pub fn save(state: &CanonicalGraphState, path: &Path) -> Result<(), PersistenceError> {
    debug!(
        node_count = state.len(),
        path = %path.display(),
        "Saving topology state to disk"
    );
    let start = std::time::Instant::now();
    let (root_id, nodes) = state.snapshot();

    let persisted = PersistedState {
        version: 1,
        root_id: root_id.map(|r| bytes_to_hex(&r)),
        nodes: nodes
            .into_iter()
            .map(|(space_id, parent_id, distance)| PersistedNode {
                space_id: bytes_to_hex(&space_id),
                parent_id: bytes_to_hex(&parent_id),
                distance,
            })
            .collect(),
    };

    let json = serde_json::to_string(&persisted)?;

    // Write to temp file then rename for atomicity
    let tmp_path = path.with_extension("tmp");

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = std::fs::File::create(&tmp_path)?;
    file.write_all(json.as_bytes())?;
    file.sync_all()?;
    drop(file);

    std::fs::rename(&tmp_path, path)?;

    let save_ms = start.elapsed().as_millis();
    debug!(
        node_count = persisted.nodes.len(),
        file_size_kb = json.len() / 1024,
        save_ms = save_ms,
        path = %path.display(),
        "Saved topology state"
    );
    Ok(())
}

/// Load graph state from a JSON file and reconstruct in-memory state.
pub fn load(path: &Path) -> Result<CanonicalGraphState, PersistenceError> {
    if !path.exists() {
        error!(path = %path.display(), "No topology state file found, starting with empty state");
        return Ok(CanonicalGraphState::new());
    }

    let start = std::time::Instant::now();
    let file_size_bytes = std::fs::metadata(path)?.len();
    let json = std::fs::read_to_string(path)?;

    let persisted: PersistedState = serde_json::from_str(&json)?;

    if persisted.version != 1 {
        warn!(
            version = persisted.version,
            "Unknown topology state version, starting fresh"
        );
        return Ok(CanonicalGraphState::new());
    }

    let root_id = match &persisted.root_id {
        Some(hex) => Some(hex_to_bytes(hex)?),
        None => None,
    };

    let mut nodes = Vec::with_capacity(persisted.nodes.len());
    for node in persisted.nodes {
        let space_id = hex_to_bytes(&node.space_id)?;
        let parent_id = hex_to_bytes(&node.parent_id)?;
        nodes.push((space_id, parent_id, node.distance));
    }

    let state = CanonicalGraphState::from_snapshot(root_id, nodes);
    let load_ms = start.elapsed().as_millis();
    info!(
        node_count = state.len(),
        file_size_kb = file_size_bytes / 1024,
        load_ms = load_ms,
        root_id = root_id.map(|r| uuid::Uuid::from_bytes(r).to_string()).unwrap_or_else(|| "none".to_string()),
        path = %path.display(),
        "Loaded topology state"
    );
    Ok(state)
}

/// Get the configured state file path.
pub fn state_path() -> std::path::PathBuf {
    std::path::PathBuf::from(
        std::env::var("TOPOLOGY_STATE_PATH")
            .unwrap_or_else(|_| DEFAULT_TOPOLOGY_STATE_PATH.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::state::{ChangeType, ParsedNodeChange};

    fn make_id(n: u8) -> [u8; 16] {
        let mut id = [0u8; 16];
        id[15] = n;
        id
    }

    #[test]
    fn test_persistence_roundtrip() {
        let dir = std::env::temp_dir().join("topology_test");
        std::fs::create_dir_all(&dir).expect("Failed to create temp dir for test");
        let path = dir.join("test_state.json");

        let state = CanonicalGraphState::new();
        let root = make_id(1);
        state.apply_changes(
            root,
            &[
                ParsedNodeChange {
                    space_id: make_id(2),
                    change_type: ChangeType::Added,
                    distance: Some(1),
                    parent_id: Some(root),
                },
                ParsedNodeChange {
                    space_id: make_id(3),
                    change_type: ChangeType::Added,
                    distance: Some(2),
                    parent_id: Some(make_id(2)),
                },
            ],
        );

        // Save
        save(&state, &path).expect("Failed to save topology state");

        // Load
        let restored = load(&path).expect("Failed to load topology state");

        assert_eq!(restored.len(), state.len());
        assert!(restored.is_canonical(&root));
        assert!(restored.is_canonical(&make_id(2)));
        assert!(restored.is_canonical(&make_id(3)));
        assert_eq!(restored.get_distance(&make_id(3)), Some(2));

        // Verify subspaces work after restore
        let subs = restored
            .get_subspaces(&root)
            .expect("Root should have subspaces after restore");
        assert_eq!(subs.len(), 3);

        // Cleanup
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_load_nonexistent() {
        let path = std::path::Path::new("/tmp/does_not_exist_topology.json");
        let state = load(path).expect("Loading nonexistent file should return Ok(empty)");
        assert!(state.is_empty());
    }

    #[test]
    fn test_hex_roundtrip() {
        let id = make_id(42);
        let hex = bytes_to_hex(&id);
        let restored = hex_to_bytes(&hex).expect("Failed to parse hex back to bytes");
        assert_eq!(id, restored);
    }
}
