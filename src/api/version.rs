use crate::AppState;
use axum::{extract::State, http::StatusCode, Json};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct VersionResponse {
    pub version: String,
}

/// Liveness of one background pipeline, read straight off its channel.
#[derive(Serialize, ToSchema)]
pub struct PipelineHealth {
    /// Live workers — receivers still holding the channel open. Zero means the
    /// pool is gone and nothing will be processed until the next restart.
    pub workers: usize,
    /// Jobs waiting for a worker.
    pub queued: usize,
    /// True once every worker is gone; enqueues fail from here on.
    pub closed: bool,
}

impl PipelineHealth {
    fn of<T>(tx: &async_channel::Sender<T>) -> Self {
        Self { workers: tx.receiver_count(), queued: tx.len(), closed: tx.is_closed() }
    }

    fn healthy(&self) -> bool {
        !self.closed && self.workers > 0
    }
}

#[derive(Serialize, ToSchema)]
pub struct HealthResponse {
    /// "ok", or "degraded" when a pipeline has lost its workers.
    pub status: &'static str,
    pub version: String,
    pub document_pipeline: PipelineHealth,
    pub geocoding_pipeline: PipelineHealth,
    pub routing_pipeline: PipelineHealth,
}

/// Service health. Unauthenticated.
///
/// Reports what `/version` cannot: whether the background pipelines still have
/// workers. A worker pool can die (a panic in a job used to be enough) while the
/// HTTP server carries on answering perfectly — from outside, the only symptom
/// was uploads that never gained a summary and enqueues that failed with
/// "sending into a closed channel". 503 when any pool is gone.
///
/// Deliberately NOT what the image's `HEALTHCHECK` probes: a dead pipeline is a
/// degraded service, not an unready one, and the container should keep serving
/// reads rather than be cycled (#404).
#[utoipa::path(
    get,
    path = "/healthz",
    tag = "meta",
    responses(
        (status = 200, description = "All pipelines have workers", body = HealthResponse),
        (status = 503, description = "A pipeline has lost every worker", body = HealthResponse)
    )
)]
pub async fn get_health(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    let document_pipeline = PipelineHealth::of(&state.pipeline_tx);
    let geocoding_pipeline = PipelineHealth::of(&state.geocoding_tx);
    let routing_pipeline = PipelineHealth::of(&state.routing_tx);
    let healthy =
        document_pipeline.healthy() && geocoding_pipeline.healthy() && routing_pipeline.healthy();
    let code = if healthy { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    (
        code,
        Json(HealthResponse {
            status: if healthy { "ok" } else { "degraded" },
            version: env!("CARGO_PKG_VERSION").to_string(),
            document_pipeline,
            geocoding_pipeline,
            routing_pipeline,
        }),
    )
}

/// Server version (matches CARGO_PKG_VERSION). Unauthenticated.
#[utoipa::path(
    get,
    path = "/version",
    tag = "meta",
    responses(
        (status = 200, description = "Server version", body = VersionResponse)
    )
)]
pub async fn get_version() -> Json<VersionResponse> {
    Json(VersionResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}
