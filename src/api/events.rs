// src/api/events.rs
use crate::{error::AppError, models::{EventListResponse, EventResponse}, AppState};
use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use axum_extra::extract::Query;
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

#[derive(Deserialize, Default, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListEventsQuery {
    /// Filter by entity UUID
    pub entity_id: Option<Uuid>,
    /// Filter by entity type (e.g. "blob", "load")
    pub entity_type: Option<String>,
    /// Filter by event type (e.g. "processing_started")
    pub event_type: Option<String>,
    /// Filter by occurred_at >= (RFC3339, e.g. 2026-01-01T00:00:00Z)
    pub from: Option<String>,
    /// Filter by occurred_at <= (RFC3339, e.g. 2026-12-31T23:59:59Z)
    pub to: Option<String>,
    /// Maximum results (default 20, max 100)
    pub limit: Option<usize>,
    /// Pagination offset (default 0)
    pub offset: Option<usize>,
}

#[utoipa::path(
    get,
    path = "/api/v1/events",
    params(ListEventsQuery),
    responses(
        (status = 200, description = "List of events", body = EventListResponse),
        (status = 400, description = "Invalid filter parameter"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "events"
)]
pub async fn list_events(
    State(state): State<AppState>,
    Query(q): Query<ListEventsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let limit = q.limit.unwrap_or(20).min(100);
    let offset = q.offset.unwrap_or(0);
    if offset > 10_000 {
        return Err(AppError::BadRequest("offset must not exceed 10000".into()));
    }
    let (_total, records) = state.db.query_events(
        q.entity_id,
        q.entity_type.as_deref(),
        q.event_type.as_deref(),
        q.from.as_deref(),
        q.to.as_deref(),
        limit,
        offset,
    ).await?;
    let items: Vec<EventResponse> = records.into_iter().map(EventResponse::from).collect();
    Ok(Json(EventListResponse { returned: items.len(), items }))
}

#[utoipa::path(
    get,
    path = "/api/v1/events/{id}",
    params(
        ("id" = Uuid, Path, description = "Event UUID"),
    ),
    responses(
        (status = 200, description = "Event record", body = EventResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "events"
)]
pub async fn get_event_handler(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_event(id).await?;
    Ok(Json(EventResponse::from(record)))
}

pub fn router() -> axum::Router<AppState> {
    use axum::routing::get;
    axum::Router::new()
        .route("/api/v1/events", get(list_events))
        .route("/api/v1/events/{id}", get(get_event_handler))
}
