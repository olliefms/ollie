// src/api/dispatchers.rs

use crate::{
    error::AppError,
    models::{DispatcherCredentials, DispatcherRecord, DispatcherStatus},
    AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateDispatcherRequest {
    pub email: String,
    pub name: String,
    pub password: String,
    /// Optional role for the new dispatcher (owner / fleet_manager / dispatcher).
    /// Defaults to `dispatcher`. The dispatcher-facing Users management surface
    /// (with owner-protection rules) lands in a later chunk (#331); this admin-key
    /// path simply provisions the record at the requested role.
    #[serde(default)]
    pub role: crate::models::permission::Role,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateDispatcherRequest {
    pub name: Option<String>,
    pub status: Option<DispatcherStatus>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ResetDispatcherPasswordRequest {
    pub password: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DispatcherListResponse {
    pub dispatchers: Vec<DispatcherRecord>,
    pub returned: usize,
}

#[utoipa::path(
    post,
    path = "/api/v1/dispatchers",
    request_body(content = CreateDispatcherRequest, description = "Dispatcher to create"),
    responses(
        (status = 201, description = "Created dispatcher record", body = DispatcherRecord),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 409, description = "Email already in use"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatchers"
)]
pub async fn create_dispatcher(
    State(state): State<AppState>,
    Json(body): Json<CreateDispatcherRequest>,
) -> Result<impl IntoResponse, AppError> {
    let email = normalize_email(&body.email);

    // Check for email uniqueness
    if state.db.get_dispatcher_by_email(&email).await?.is_some() {
        return Err(AppError::Conflict("email already in use".into()));
    }

    let now = Utc::now();
    let id = Uuid::new_v4();

    let record = DispatcherRecord {
        id,
        email,
        name: body.name,
        status: DispatcherStatus::Active,
        role: body.role,
        extra_scopes: Vec::new(),
        created_at: now,
        updated_at: now,
    };

    state.db.insert_dispatcher(&record).await?;

    let password = body.password;
    let password_hash = tokio::task::spawn_blocking(move || bcrypt::hash(&password, 12u32))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let creds = DispatcherCredentials {
        dispatcher_id: id,
        password_hash,
        token_version: 0,
        failed_attempts: 0,
        locked_until: None,
        updated_at: now,
    };
    state.db.upsert_dispatcher_credentials(&creds).await?;

    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(
    get,
    path = "/api/v1/dispatchers",
    responses(
        (status = 200, description = "List of dispatchers", body = DispatcherListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatchers"
)]
pub async fn list_dispatchers(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let dispatchers = state.db.list_dispatchers().await?;
    let returned = dispatchers.len();
    Ok(Json(DispatcherListResponse { dispatchers, returned }))
}

#[utoipa::path(
    get,
    path = "/api/v1/dispatchers/{id}",
    params(
        ("id" = Uuid, Path, description = "Dispatcher UUID")
    ),
    responses(
        (status = 200, description = "Dispatcher record", body = DispatcherRecord),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatchers"
)]
pub async fn get_dispatcher(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_dispatcher_by_id(id).await?;
    Ok(Json(record))
}

#[utoipa::path(
    put,
    path = "/api/v1/dispatchers/{id}",
    params(
        ("id" = Uuid, Path, description = "Dispatcher UUID")
    ),
    request_body(content = UpdateDispatcherRequest, description = "Fields to update — all optional"),
    responses(
        (status = 200, description = "Updated dispatcher record", body = DispatcherRecord),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatchers"
)]
pub async fn update_dispatcher(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateDispatcherRequest>,
) -> Result<impl IntoResponse, AppError> {
    let mut record = state.db.get_dispatcher_by_id(id).await?;
    if let Some(name) = body.name {
        record.name = name;
    }
    if let Some(status) = body.status {
        record.status = status;
    }
    record.updated_at = Utc::now();
    state.db.upsert_dispatcher(&record).await?;
    Ok(Json(record))
}

#[utoipa::path(
    put,
    path = "/api/v1/dispatchers/{id}/password",
    params(
        ("id" = Uuid, Path, description = "Dispatcher UUID")
    ),
    request_body(content = ResetDispatcherPasswordRequest, description = "New password"),
    responses(
        (status = 204, description = "Password reset successfully"),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatchers"
)]
pub async fn reset_dispatcher_password(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<ResetDispatcherPasswordRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Verify dispatcher exists
    state.db.get_dispatcher_by_id(id).await?;

    let password = body.password;
    let password_hash = tokio::task::spawn_blocking(move || bcrypt::hash(&password, 12u32))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let now = Utc::now();
    let new_token_version = match state.db.get_dispatcher_credentials(id).await? {
        Some(existing) => existing.token_version + 1,
        None => 0,
    };

    let creds = DispatcherCredentials {
        dispatcher_id: id,
        password_hash,
        token_version: new_token_version,
        failed_attempts: 0,
        locked_until: None,
        updated_at: now,
    };
    state.db.upsert_dispatcher_credentials(&creds).await?;

    Ok(StatusCode::NO_CONTENT)
}

pub fn router() -> Router<AppState> {
    use axum::routing::{get, post, put};
    Router::new()
        .route("/api/v1/dispatchers", post(create_dispatcher))
        .route("/api/v1/dispatchers", get(list_dispatchers))
        .route("/api/v1/dispatchers/{id}", get(get_dispatcher))
        .route("/api/v1/dispatchers/{id}", put(update_dispatcher))
        .route("/api/v1/dispatchers/{id}/password", put(reset_dispatcher_password))
}
