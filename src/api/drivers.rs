// src/api/drivers.rs
use crate::{
    ai::embed::embed_text,
    error::AppError,
    models::{
        CreateDriverRequest, DriverListResponse, DriverRecord, DriverStatus,
        UpdateDriverRequest,
    },
    AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
    Router,
};
use axum_extra::extract::Query;
use chrono::Utc;
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

#[derive(Deserialize, Default, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListDriversQuery {
    /// Semantic search query — triggers vector search when present
    pub s: Option<String>,
    /// Filter by status (available, assigned, dispatched, inactive)
    pub status: Option<String>,
    /// Maximum results (default 20, max 100)
    pub limit: Option<usize>,
    /// Pagination offset (default 0)
    pub offset: Option<usize>,
}

#[utoipa::path(
    post,
    path = "/api/v1/drivers",
    request_body(content = CreateDriverRequest, description = "Driver to create"),
    responses(
        (status = 201, description = "Created driver record", body = DriverRecord),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "drivers"
)]
pub async fn create_driver(
    State(state): State<AppState>,
    Json(body): Json<CreateDriverRequest>,
) -> Result<impl IntoResponse, AppError> {
    let now = Utc::now();

    let record = DriverRecord {
        id: Uuid::new_v4(),
        name: body.name,
        phone: body.phone,
        email: body.email,
        license_number: body.license_number,
        license_state: body.license_state,
        license_expiry: body.license_expiry,
        status: DriverStatus::Available,
        notes: body.notes,
        embedding: None,
        owner_id: 0,
        created_at: now,
        updated_at: now,
    };

    let embedding = embed_text(&state.ai, &record.embedding_text()).await.ok();
    let record = DriverRecord { embedding, ..record };

    state.db.insert_driver(&record).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(
    get,
    path = "/api/v1/drivers",
    params(ListDriversQuery),
    responses(
        (status = 200, description = "List of drivers (or semantic search results when ?s= is provided)", body = DriverListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "drivers"
)]
pub async fn list_drivers(
    State(state): State<AppState>,
    Query(q): Query<ListDriversQuery>,
) -> Result<impl IntoResponse, AppError> {
    let limit = q.limit.unwrap_or(20).min(100);
    let offset = q.offset.unwrap_or(0);

    if let Some(query_text) = q.s {
        let embedding = embed_text(&state.ai, &query_text).await?;
        let items = state.db.search_drivers(embedding, q.status.as_deref(), limit).await?;
        let returned = items.len();
        return Ok(Json(DriverListResponse { returned, items }));
    }

    let (total, items) = state.db.list_drivers(q.status.as_deref(), limit, offset).await?;
    Ok(Json(DriverListResponse { returned: total, items }))
}

#[utoipa::path(
    get,
    path = "/api/v1/drivers/{id}",
    params(
        ("id" = Uuid, Path, description = "Driver UUID")
    ),
    responses(
        (status = 200, description = "Driver record", body = DriverRecord),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "drivers"
)]
pub async fn get_driver(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_driver_by_id(id).await?;
    Ok(Json(record))
}

#[utoipa::path(
    put,
    path = "/api/v1/drivers/{id}",
    params(
        ("id" = Uuid, Path, description = "Driver UUID")
    ),
    request_body(content = UpdateDriverRequest, description = "Fields to update — all optional. Cannot set status to assigned or dispatched."),
    responses(
        (status = 200, description = "Updated driver record", body = DriverRecord),
        (status = 400, description = "Bad request — cannot manually set assigned or dispatched"),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "drivers"
)]
pub async fn update_driver(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateDriverRequest>,
) -> Result<impl IntoResponse, AppError> {
    let updated = state.db.update_driver_metadata(
        id,
        body.name,
        body.phone,
        body.email,
        body.license_number,
        body.license_state,
        body.license_expiry,
        body.notes,
    ).await?;

    if let Ok(embedding) = embed_text(&state.ai, &updated.embedding_text()).await {
        let _ = state.db.update_driver_embedding(id, embedding).await;
    }

    Ok(Json(updated))
}

#[utoipa::path(
    delete,
    path = "/api/v1/drivers/{id}",
    params(
        ("id" = Uuid, Path, description = "Driver UUID")
    ),
    responses(
        (status = 204, description = "Soft-deleted (status set to inactive)"),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "drivers"
)]
pub async fn delete_driver(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    state.db.soft_delete_driver(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn router() -> Router<AppState> {
    use axum::routing::{delete, get, post, put};
    Router::new()
        .route("/api/v1/drivers", post(create_driver))
        .route("/api/v1/drivers", get(list_drivers))
        .route("/api/v1/drivers/:id", get(get_driver))
        .route("/api/v1/drivers/:id", put(update_driver))
        .route("/api/v1/drivers/:id", delete(delete_driver))
}
