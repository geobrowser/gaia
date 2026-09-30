use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::{Router, extract::State, http::StatusCode, routing::get};
use tracing::{error, info};

#[derive(Clone, Default)]
pub struct Health {
    pub ready: Arc<AtomicBool>,
    pub degraded: Arc<AtomicBool>,
}

impl Health {
    pub fn set_ready(&self, v: bool) {
        self.ready.store(v, Ordering::Relaxed);
    }
    pub fn set_degraded(&self, v: bool) {
        self.degraded.store(v, Ordering::Relaxed);
    }
}

async fn ready(State(h): State<Health>) -> StatusCode {
    if h.ready.load(Ordering::Relaxed) && !h.degraded.load(Ordering::Relaxed) {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

pub fn spawn(health: Health, port: u16) -> tokio::task::JoinHandle<()> {
    let app = Router::new()
        .route("/health/live", get(|| async { StatusCode::OK }))
        .route("/health/ready", get(ready))
        .with_state(health);
    tokio::spawn(async move {
        let addr = format!("0.0.0.0:{port}");
        match tokio::net::TcpListener::bind(&addr).await {
            Ok(listener) => {
                info!(addr = %addr, "health server listening");
                if let Err(e) = axum::serve(listener, app).await {
                    error!(error = %e, "health server error");
                }
            }
            Err(e) => error!(error = %e, addr = %addr, "failed to bind health server"),
        }
    })
}
