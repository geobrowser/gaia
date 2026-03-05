//! Topology query handlers for canonical graph introspection.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};

use super::AppState;

/// Parse a space_id from a path parameter.
/// Accepts both dashed UUID (36 chars) and 32-char hex.
fn parse_space_id(space_id_str: &str) -> Option<[u8; 16]> {
    // Uuid::parse_str accepts both dashed (36-char) and simple hex (32-char) formats.
    uuid::Uuid::parse_str(space_id_str)
        .ok()
        .map(|uuid| *uuid.as_bytes())
}

/// GET /topology/subspaces/:space_id
///
/// Returns all subspaces (descendants + self) for a canonical space.
async fn get_subspaces(
    State(state): State<AppState>,
    Path(space_id_str): Path<String>,
) -> impl IntoResponse {
    let space_bytes = match parse_space_id(&space_id_str) {
        Some(bytes) => bytes,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "invalid space_id format" })),
            );
        }
    };

    match state.topology_state.get_subspaces(&space_bytes) {
        Some(subspaces) => {
            let count = subspaces.len();
            let subspace_strs: Vec<String> = subspaces.iter().map(|u| u.to_string()).collect();
            let is_root = state
                .topology_state
                .root_id()
                .map(|r| *r.as_bytes() == space_bytes)
                .unwrap_or(false);
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "space_id": space_id_str,
                    "subspaces": subspace_strs,
                    "count": count,
                    "is_root": is_root,
                })),
            )
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "space not found in canonical graph",
                "space_id": space_id_str,
            })),
        ),
    }
}

/// GET /topology/distance/:space_id
///
/// Returns the distance from root for a canonical space.
async fn get_distance(
    State(state): State<AppState>,
    Path(space_id_str): Path<String>,
) -> impl IntoResponse {
    let space_bytes = match parse_space_id(&space_id_str) {
        Some(bytes) => bytes,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "invalid space_id format" })),
            );
        }
    };

    match state.topology_state.get_distance(&space_bytes) {
        Some(distance) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "space_id": space_id_str,
                "distance": distance,
            })),
        ),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "space not found in canonical graph",
                "space_id": space_id_str,
            })),
        ),
    }
}

/// GET /topology/root
///
/// Returns the canonical graph root space ID.
async fn get_root(State(state): State<AppState>) -> impl IntoResponse {
    match state.topology_state.root_id() {
        Some(root_uuid) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "root_id": root_uuid.to_string(),
            })),
        ),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "no root set yet",
            })),
        ),
    }
}

/// Topology routes: `/topology/root`, `/topology/subspaces/:id`, `/topology/distance/:id`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/topology/subspaces/{space_id}", get(get_subspaces))
        .route("/topology/distance/{space_id}", get(get_distance))
        .route("/topology/root", get(get_root))
}
