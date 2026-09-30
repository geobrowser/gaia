//! HTTP surface and the loaded-slot registry.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use embedding::{Descriptor, EmbeddingProvider, OnnxLocalProvider, OnnxOptions, Purpose, bundle};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tracing::{error, info, warn};

use crate::config::ServeArgs;

pub struct LoadedSlot {
    pub slot: String,
    pub hash: String,
    pub descriptor: Descriptor,
    pub provider: Arc<OnnxLocalProvider>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Limits {
    pub max_batch: usize,
    pub max_text_chars: usize,
    pub max_inflight: usize,
    pub query_inflight: usize,
    pub batch_size: usize,
    pub intra_threads: Option<usize>,
    pub query_lane: bool,
}

impl From<&ServeArgs> for Limits {
    fn from(a: &ServeArgs) -> Self {
        Self {
            max_batch: a.max_batch,
            max_text_chars: a.max_text_chars,
            max_inflight: a.max_inflight,
            query_inflight: a.query_inflight,
            batch_size: a.batch_size,
            intra_threads: a.intra_threads,
            query_lane: a.query_lane,
        }
    }
}

pub struct AppState {
    slots: BTreeMap<String, Arc<LoadedSlot>>,
    limits: Limits,
    documents: Semaphore,
    queries: Semaphore,
    started: Instant,
}

impl AppState {
    pub fn new(slots: Vec<LoadedSlot>, limits: Limits) -> Self {
        Self {
            slots: slots
                .into_iter()
                .map(|s| (s.slot.clone(), Arc::new(s)))
                .collect(),
            documents: Semaphore::new(limits.max_inflight.max(1)),
            queries: Semaphore::new(limits.query_inflight.max(1)),
            limits,
            started: Instant::now(),
        }
    }

    pub fn slot_ids(&self) -> Vec<String> {
        self.slots.keys().cloned().collect()
    }
}

/// Load every requested bundle under `models_dir` (all of them when `wanted` is empty).
pub fn load_slots(args: &ServeArgs) -> anyhow::Result<Vec<LoadedSlot>> {
    let options = OnnxOptions {
        intra_threads: args.intra_threads,
        batch_size: args.batch_size,
        query_lane: args.query_lane,
    };
    let wanted: Vec<String> = args
        .slots
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect();
    let dirs: Vec<std::path::PathBuf> = if wanted.is_empty() {
        bundle::discover(&args.models_dir)?
    } else {
        wanted.iter().map(|s| args.models_dir.join(s)).collect()
    };
    if dirs.is_empty() {
        anyhow::bail!("no bundles found under {}", args.models_dir.display());
    }
    let mut loaded = Vec::new();
    for dir in dirs {
        loaded.push(load_slot(&dir, options.clone())?);
    }
    Ok(loaded)
}

pub fn load_slot(dir: &Path, options: OnnxOptions) -> anyhow::Result<LoadedSlot> {
    let t = Instant::now();
    let (verified, provider) = bundle::load(dir, options)?;
    info!(
        slot = %verified.slot,
        descriptor_hash = %verified.hash,
        model_id = %verified.descriptor.model_id,
        dimensions = verified.descriptor.dimensions,
        query_lane = provider.has_query_lane(),
        took_ms = t.elapsed().as_millis() as u64,
        dir = %dir.display(),
        "bundle loaded"
    );
    Ok(LoadedSlot {
        slot: verified.slot,
        hash: verified.hash,
        descriptor: verified.descriptor,
        provider: Arc::new(provider),
    })
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/embed", post(embed))
        .route("/info", get(info_handler))
        .route("/health/live", get(|| async { StatusCode::OK }))
        .route("/health/ready", get(ready))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
pub struct EmbedRequest {
    pub slot: String,
    pub purpose: Purpose,
    pub texts: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct EmbedResponse {
    pub slot: String,
    pub descriptor_hash: String,
    pub dimensions: usize,
    pub purpose: Purpose,
    pub vectors: Vec<Vec<f32>>,
    pub took_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct ApiError {
    #[serde(skip)]
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": { "code": self.code, "message": self.message } })),
        )
            .into_response()
    }
}

async fn embed(
    State(state): State<Arc<AppState>>,
    body: Result<Json<EmbedRequest>, JsonRejection>,
) -> Result<Json<EmbedResponse>, ApiError> {
    // A malformed body gets the same `{ "error": { code, message } }` shape as every other
    // failure instead of axum's plain-text rejection.
    let Json(req) = body.map_err(|e| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_body",
            e.body_text(),
        )
    })?;
    let limits = &state.limits;
    if req.texts.len() > limits.max_batch {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "batch_too_large",
            format!("{} texts, limit {}", req.texts.len(), limits.max_batch),
        ));
    }
    if let Some((i, t)) = req
        .texts
        .iter()
        .enumerate()
        .find(|(_, t)| t.chars().count() > limits.max_text_chars)
    {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "text_too_long",
            format!(
                "texts[{i}] has {} chars, limit {}",
                t.chars().count(),
                limits.max_text_chars
            ),
        ));
    }
    let slot = state.slots.get(&req.slot).cloned().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "unknown_slot",
            format!("slot {:?} is not loaded", req.slot),
        )
    })?;

    let lane = match req.purpose {
        Purpose::Query => &state.queries,
        Purpose::Document => &state.documents,
    };
    let _permit = lane.acquire().await.map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "shutting_down",
            "service is shutting down",
        )
    })?;

    let started = Instant::now();
    let n = req.texts.len();
    let purpose = req.purpose;
    let provider = slot.provider.clone();
    let texts = req.texts;
    let vectors = tokio::task::spawn_blocking(move || provider.embed(&texts, purpose))
        .await
        .map_err(|e| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "embedding_panicked",
                e.to_string(),
            )
        })?
        .map_err(|e| {
            error!(slot = %slot.slot, error = %e, "embedding failed");
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "embedding_failed",
                e.to_string(),
            )
        })?;
    let took_ms = started.elapsed().as_millis() as u64;
    info!(slot = %slot.slot, purpose = ?purpose, texts = n, took_ms, "embedded");
    Ok(Json(EmbedResponse {
        slot: slot.slot.clone(),
        descriptor_hash: slot.hash.clone(),
        dimensions: slot.descriptor.dimensions,
        purpose,
        vectors,
        took_ms,
    }))
}

#[derive(Serialize)]
struct SlotInfo<'a> {
    descriptor: &'a Descriptor,
    descriptor_hash: &'a str,
    vector_field: String,
}

async fn info_handler(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let slots: BTreeMap<&str, SlotInfo> = state
        .slots
        .iter()
        .map(|(id, s)| {
            (
                id.as_str(),
                SlotInfo {
                    descriptor: &s.descriptor,
                    descriptor_hash: &s.hash,
                    vector_field: s.descriptor.vector_field(),
                },
            )
        })
        .collect();
    Json(serde_json::json!({
        "service": { "name": "embedding-service", "version": env!("CARGO_PKG_VERSION"), "uptime_s": state.started.elapsed().as_secs() },
        "slots": slots,
        "limits": state.limits,
    }))
}

async fn ready(State(state): State<Arc<AppState>>) -> Response {
    if state.slots.is_empty() {
        warn!("readiness: no slots loaded");
        return (StatusCode::SERVICE_UNAVAILABLE, "no slots loaded").into_response();
    }
    StatusCode::OK.into_response()
}

/// Bind and serve until SIGTERM / Ctrl-C.
pub async fn serve(state: Arc<AppState>, bind: &str, port: u16) -> anyhow::Result<()> {
    let addr = format!("{bind}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!(addr = %addr, slots = ?state.slot_ids(), "embedding-service listening");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => error!(error = %e, "failed to install SIGTERM handler"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    info!("shutdown signal received");
}
