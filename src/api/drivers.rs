// src/api/drivers.rs
fn normalize_phone(phone: &str) -> String {
    let stripped: String = phone.chars()
        .filter(|c| !matches!(c, ' ' | '-' | '(' | ')'))
        .collect();
    if stripped.starts_with('+') {
        return stripped;
    }
    if stripped.len() == 10 && stripped.chars().all(|c| c.is_ascii_digit()) {
        return format!("+1{stripped}");
    }
    if stripped.chars().all(|c| c.is_ascii_digit()) {
        return format!("+{stripped}");
    }
    stripped
}

use crate::{
    ai::embed::embed_text,
    error::AppError,
    models::{
        CreateDriverRequest, DriverCredentials, DriverListResponse, DriverRecord, DriverStatus,
        SetDriverPinRequest, UpdateDriverRequest,
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
        current_truck_id: None,
        current_trailer_ids: vec![],
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
    let phone = body.phone.as_deref().map(normalize_phone);
    let updated = state.db.update_driver_metadata(
        id,
        body.name,
        phone,
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
    // Invalidate any outstanding JWTs
    if let Ok(Some(mut creds)) = state.db.get_driver_credentials(id).await {
        creds.token_version += 1;
        creds.updated_at = chrono::Utc::now();
        let _ = state.db.upsert_driver_credentials(&creds).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/drivers/{id}/pin",
    tag = "drivers",
    request_body = SetDriverPinRequest,
    responses(
        (status = 204, description = "PIN set successfully"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Driver not found"),
        (status = 422, description = "Invalid PIN format"),
        (status = 500, description = "Internal server error"),
    ),
    security(("BearerAuth" = [])),
)]
pub async fn set_driver_pin(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetDriverPinRequest>,
) -> Result<impl IntoResponse, AppError> {
    state.db.get_driver_by_id(id).await?;

    let pin = body.pin.trim().to_string();
    let len = pin.len();
    if !(4..=6).contains(&len) || !pin.chars().all(|c| c.is_ascii_digit()) {
        return Err(AppError::UnprocessableEntity("PIN must be 4–6 numeric digits".into()));
    }

    let pin_hash = tokio::task::spawn_blocking(move || bcrypt::hash(&pin, 12u32))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let now = Utc::now();
    let credentials = match state.db.get_driver_credentials(id).await? {
        // Non-atomic: fetch → increment → upsert means two concurrent PIN resets could produce the same new version.
        // Acceptable here — PIN resets are rare and have no tight concurrent requirements.
        Some(existing) => DriverCredentials {
            driver_id: id,
            pin_hash: Some(pin_hash),
            token_version: existing.token_version + 1,
            failed_pin_attempts: 0,
            locked_until: None,
            updated_at: now,
        },
        // token_version starts at 0 and is incremented on each PIN change to invalidate outstanding JWTs.
        None => DriverCredentials {
            driver_id: id,
            pin_hash: Some(pin_hash),
            token_version: 0,
            failed_pin_attempts: 0,
            locked_until: None,
            updated_at: now,
        },
    };

    state.db.upsert_driver_credentials(&credentials).await?;
    tracing::info!(driver_id = %id, "dispatcher set PIN for driver");
    Ok(StatusCode::NO_CONTENT)
}

pub fn router() -> Router<AppState> {
    use axum::routing::{delete, get, post, put};
    Router::new()
        .route("/api/v1/drivers", post(create_driver))
        .route("/api/v1/drivers", get(list_drivers))
        .route("/api/v1/drivers/{id}", get(get_driver))
        .route("/api/v1/drivers/{id}", put(update_driver))
        .route("/api/v1/drivers/{id}", delete(delete_driver))
        .route("/api/v1/drivers/{id}/pin", post(set_driver_pin))
}
