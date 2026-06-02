// src/api/fleet_portal/trailer_writes.rs
//
// Fleet-portal trailer write endpoints (#269):
//   - POST  /fleet/api/v1/trailers
//   - PATCH /fleet/api/v1/trailers/{id}
//
// Mirrors the admin behaviour in `src/api/trailers.rs` but exposes it under
// the fleet user JWT instead of the admin Bearer key. The `apply_*` helpers
// are shared with the MCP tools so validation and side effects (embedding
// refresh) stay in one place. `status` cannot be set on create or via PATCH
// through these endpoints — the fleet_user cannot manually transition a
// trailer to `assigned`/`dispatched`; those are owned by the trip lifecycle.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use super::jwt::FleetUserClaims;
use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    ai::embed::embed_text,
    error::AppError,
    models::{TrailerOwner, TrailerRecord, TrailerStatus},
    AppState,
};

/// Strict create body — rejects unknown fields so fleet_user agents can't
/// accidentally pass admin-only or stale fields (e.g. `status`, `embedding`,
/// `owner_id`) and have them silently ignored.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTrailerBody {
    pub unit_number: String,
    pub owner: TrailerOwner,
    #[serde(default)]
    pub owner_name: Option<String>,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub make: Option<String>,
    #[serde(default)]
    pub trailer_type: Option<String>,
    #[serde(default)]
    pub length_ft: Option<f64>,
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

/// Strict patch body — rejects unknown fields. All fields optional; omitted
/// fields are left unchanged. `status` is intentionally omitted: fleet_user
/// agents cannot manually change a trailer's lifecycle status.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchTrailerBody {
    #[serde(default)]
    pub unit_number: Option<String>,
    #[serde(default)]
    pub owner: Option<TrailerOwner>,
    #[serde(default)]
    pub owner_name: Option<String>,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub make: Option<String>,
    #[serde(default)]
    pub trailer_type: Option<String>,
    #[serde(default)]
    pub length_ft: Option<f64>,
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
    path = "/fleet/api/v1/trailers",
    request_body(content = CreateTrailerBody, description = "Trailer to create"),
    responses(
        (status = 201, description = "Created trailer record", body = TrailerRecord),
        (status = 400, description = "Bad request — unknown field, missing owner_name for non-fleet, or invalid body"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn create_trailer_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trailers:write")?;
    let record = apply_trailer_create(&state, body).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(
    patch,
    path = "/fleet/api/v1/trailers/{id}",
    params(("id" = Uuid, Path, description = "Trailer UUID")),
    request_body(content = PatchTrailerBody, description = "Fields to update — all optional"),
    responses(
        (status = 200, description = "Updated trailer record", body = TrailerRecord),
        (status = 400, description = "Bad request — unknown field or invalid body"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Trailer not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn update_trailer_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trailers:write")?;
    let record = apply_trailer_patch(&state, id, body).await?;
    Ok(Json(record))
}

#[utoipa::path(
    delete,
    path = "/fleet/api/v1/trailers/{id}",
    params(("id" = Uuid, Path, description = "Trailer UUID")),
    responses(
        (status = 204, description = "Soft-deleted (status set to inactive)"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Trailer not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn delete_trailer_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trailers:delete")?;
    state.db.soft_delete_trailer(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Shared create writer — used by the HTTP handler and the MCP `create_trailer`
/// tool. Validates input, generates the embedding (best-effort), persists the
/// record. New trailers start in `Available` status (matches admin behaviour).
pub async fn apply_trailer_create(
    state: &AppState,
    body: Value,
) -> Result<TrailerRecord, AppError> {
    let parsed: CreateTrailerBody = serde_json::from_value(body)
        .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))?;

    if parsed.unit_number.trim().is_empty() {
        return Err(AppError::BadRequest("unit_number is required".into()));
    }
    if parsed.owner != TrailerOwner::Fleet && parsed.owner_name.is_none() {
        return Err(AppError::BadRequest(
            "owner_name is required when owner is not fleet".into(),
        ));
    }

    let now = Utc::now();
    let record = TrailerRecord {
        id: Uuid::new_v4(),
        unit_number: parsed.unit_number,
        owner: parsed.owner,
        owner_name: parsed.owner_name,
        year: parsed.year,
        make: parsed.make,
        trailer_type: parsed.trailer_type,
        length_ft: parsed.length_ft,
        vin: parsed.vin,
        plate: parsed.plate,
        plate_state: parsed.plate_state,
        status: TrailerStatus::Available,
        notes: parsed.notes,
        blob_ids: parsed.blob_ids,
        embedding: None,
        owner_id: 0,
        created_at: now,
        updated_at: now,
    };

    let embedding = embed_text(&state.ai, &record.embedding_text()).await.ok();
    let record = TrailerRecord { embedding, ..record };

    state.db.insert_trailer(&record).await?;
    Ok(record)
}

/// Shared patch writer — used by the HTTP handler and the MCP `update_trailer`
/// tool. Updates allowed metadata fields; refreshes the embedding best-effort.
/// `status` is not exposed through this path.
pub async fn apply_trailer_patch(
    state: &AppState,
    id: Uuid,
    body: Value,
) -> Result<TrailerRecord, AppError> {
    let parsed: PatchTrailerBody = serde_json::from_value(body)
        .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))?;

    if let Some(ref n) = parsed.unit_number {
        if n.trim().is_empty() {
            return Err(AppError::BadRequest("unit_number cannot be empty".into()));
        }
    }

    let updated = state.db.update_trailer_metadata(
        id,
        parsed.unit_number,
        parsed.owner,
        parsed.owner_name,
        parsed.year,
        parsed.make,
        parsed.trailer_type,
        parsed.length_ft,
        parsed.vin,
        parsed.plate,
        parsed.plate_state,
        parsed.notes,
        parsed.blob_ids,
    ).await?;

    if let Ok(embedding) = embed_text(&state.ai, &updated.embedding_text()).await {
        let _ = state.db.update_trailer_embedding(id, embedding).await;
    }

    Ok(updated)
}
