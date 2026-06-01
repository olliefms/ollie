// src/api/dispatcher_portal/truck_writes.rs
//
// Dispatcher-portal truck write endpoints (#269):
//   - POST  /dispatch/api/v1/trucks
//   - PATCH /dispatch/api/v1/trucks/{id}
//
// Mirrors the admin behaviour in `src/api/trucks.rs` but exposes it under the
// dispatcher JWT. The `apply_*` helpers are shared with the MCP tools so HTTP
// and MCP enforce the same validation and embedding-refresh behaviour.
// `status` is not exposed through these endpoints — trucks transition to
// `assigned`/`dispatched` only via the trip lifecycle.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use super::jwt::DispatcherClaims;
use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    ai::embed::embed_text,
    error::AppError,
    models::{TruckRecord, TruckStatus},
    AppState,
};

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTruckBody {
    pub unit_number: String,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub make: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub vin: Option<String>,
    #[serde(default)]
    pub plate: Option<String>,
    #[serde(default)]
    pub plate_state: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchTruckBody {
    #[serde(default)]
    pub unit_number: Option<String>,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub make: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub vin: Option<String>,
    #[serde(default)]
    pub plate: Option<String>,
    #[serde(default)]
    pub plate_state: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub blob_ids: Option<Vec<Uuid>>,
}

#[utoipa::path(
    post,
    path = "/dispatch/api/v1/trucks",
    request_body(content = CreateTruckBody, description = "Truck to create"),
    responses(
        (status = 201, description = "Created truck record", body = TruckRecord),
        (status = 400, description = "Bad request — unknown field or invalid body"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn create_truck_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trucks:write")?;
    let record = apply_truck_create(&state, body).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(
    patch,
    path = "/dispatch/api/v1/trucks/{id}",
    params(("id" = Uuid, Path, description = "Truck UUID")),
    request_body(content = PatchTruckBody, description = "Fields to update — all optional"),
    responses(
        (status = 200, description = "Updated truck record", body = TruckRecord),
        (status = 400, description = "Bad request — unknown field or invalid body"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Truck not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn update_truck_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trucks:write")?;
    let record = apply_truck_patch(&state, id, body).await?;
    Ok(Json(record))
}

#[utoipa::path(
    delete,
    path = "/dispatch/api/v1/trucks/{id}",
    params(("id" = Uuid, Path, description = "Truck UUID")),
    responses(
        (status = 204, description = "Soft-deleted (status set to inactive)"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Truck not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn delete_truck_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trucks:delete")?;
    state.db.soft_delete_truck(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn apply_truck_create(
    state: &AppState,
    body: Value,
) -> Result<TruckRecord, AppError> {
    let parsed: CreateTruckBody = serde_json::from_value(body)
        .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))?;

    if parsed.unit_number.trim().is_empty() {
        return Err(AppError::BadRequest("unit_number is required".into()));
    }

    let now = Utc::now();
    let record = TruckRecord {
        id: Uuid::new_v4(),
        unit_number: parsed.unit_number,
        year: parsed.year,
        make: parsed.make,
        model: parsed.model,
        vin: parsed.vin,
        plate: parsed.plate,
        plate_state: parsed.plate_state,
        status: TruckStatus::Available,
        notes: parsed.notes,
        blob_ids: parsed.blob_ids,
        embedding: None,
        owner_id: 0,
        created_at: now,
        updated_at: now,
    };

    let embedding = embed_text(&state.ai, &record.embedding_text()).await.ok();
    let record = TruckRecord { embedding, ..record };

    state.db.insert_truck(&record).await?;
    Ok(record)
}

pub async fn apply_truck_patch(
    state: &AppState,
    id: Uuid,
    body: Value,
) -> Result<TruckRecord, AppError> {
    let parsed: PatchTruckBody = serde_json::from_value(body)
        .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))?;

    if let Some(ref n) = parsed.unit_number {
        if n.trim().is_empty() {
            return Err(AppError::BadRequest("unit_number cannot be empty".into()));
        }
    }

    let updated = state.db.update_truck_metadata(
        id,
        parsed.unit_number,
        parsed.year,
        parsed.make,
        parsed.model,
        parsed.vin,
        parsed.plate,
        parsed.plate_state,
        parsed.notes,
        parsed.blob_ids,
    ).await?;

    if let Ok(embedding) = embed_text(&state.ai, &updated.embedding_text()).await {
        let _ = state.db.update_truck_embedding(id, embedding).await;
    }

    Ok(updated)
}
