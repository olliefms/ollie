// src/api/fleet_portal/facility_writes.rs
//
// Fleet-portal facility write endpoints (#265):
//   - POST  /fleet/api/v1/facilities
//   - PATCH /fleet/api/v1/facilities/{id}
//
// Mirrors the admin behaviour in `src/api/facilities.rs` but exposes it under
// the fleet user JWT instead of the admin Bearer key. The `apply_*` helpers
// are shared with the MCP tools so validation and side effects (embedding
// refresh, geocode re-queue, manual-coords override) stay in one place.

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
    api::utils::referrer_conflict_message,
    error::AppError,
    models::{
        validate_coords, FacilityContact, FacilityRecord, GeocodeStatus,
    },
    AppState,
};

/// Strict create body — rejects unknown fields so fleet_user agents can't
/// accidentally pass admin-only or stale fields and have them silently
/// ignored. Mirrors the field set of `models::CreateFacilityRequest`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateFacilityBody {
    pub name: String,
    pub address: String,
    #[serde(default)]
    pub contacts: Vec<FacilityContact>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
    #[serde(default)]
    pub lat: Option<f64>,
    #[serde(default)]
    pub lng: Option<f64>,
}

/// Strict patch body — rejects unknown fields. All fields are optional;
/// omitted fields are left unchanged.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchFacilityBody {
    pub name: Option<String>,
    pub address: Option<String>,
    pub contacts: Option<Vec<FacilityContact>>,
    pub notes: Option<String>,
    pub tags: Option<Vec<String>>,
    pub blob_ids: Option<Vec<Uuid>>,
    #[serde(default)]
    pub lat: Option<f64>,
    #[serde(default)]
    pub lng: Option<f64>,
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/facilities",
    request_body(content = CreateFacilityBody, description = "Facility to create"),
    responses(
        (status = 201, description = "Created facility record", body = FacilityRecord),
        (status = 400, description = "Bad request — unknown field or invalid body"),
        (status = 401, description = "Unauthorized"),
        (status = 422, description = "Invalid coordinates"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn create_facility_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("facilities:write")?;
    let record = apply_facility_create(&state, body).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(
    patch,
    path = "/fleet/api/v1/facilities/{id}",
    params(("id" = Uuid, Path, description = "Facility UUID")),
    request_body(content = PatchFacilityBody, description = "Fields to update — all optional"),
    responses(
        (status = 200, description = "Updated facility record", body = FacilityRecord),
        (status = 400, description = "Bad request — unknown field or invalid body"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Facility not found"),
        (status = 422, description = "Invalid coordinates"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn update_facility_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("facilities:write")?;
    let record = apply_facility_patch(&state, id, body).await?;
    Ok(Json(record))
}

/// Shared create writer — used by the HTTP handler and the MCP `create_facility`
/// tool. Validates input, generates the embedding (best-effort), persists the
/// record, and pushes to the geocoding queue when no manual coords are given.
pub async fn apply_facility_create(
    state: &AppState,
    body: Value,
) -> Result<FacilityRecord, AppError> {
    let parsed: CreateFacilityBody = serde_json::from_value(body)
        .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))?;

    let coords = validate_coords(parsed.lat, parsed.lng)?;
    let (lat, lng, status) = match coords {
        Some((la, lo)) => (Some(la), Some(lo), GeocodeStatus::Ready),
        None => (None, None, GeocodeStatus::Pending),
    };

    let embed_input = format!(
        "{} {} {} {}",
        parsed.name,
        parsed.address,
        parsed.notes.as_deref().unwrap_or(""),
        parsed.tags.join(" "),
    );
    let embedding = embed_text(&state.ai, &embed_input).await.ok();

    let now = Utc::now();
    let record = FacilityRecord {
        id: Uuid::new_v4(),
        owner_id: 0,
        name: parsed.name,
        address: parsed.address,
        normalized_address: None,
        lat,
        lng,
        geocode_status: status,
        geocode_failure_count: 0,
        contacts: parsed.contacts,
        notes: parsed.notes,
        tags: parsed.tags,
        blob_ids: parsed.blob_ids,
        avg_dwell_minutes: None,
        dwell_sample_count: 0,
        archived: false,
        embedding,
        created_at: now,
        updated_at: now,
    };

    state.db.insert_facility(&record).await?;
    if coords.is_none() {
        let _ = state.geocoding_tx.try_send(record.id);
    }

    Ok(record)
}

/// Shared patch writer — used by the HTTP handler and the MCP `update_facility`
/// tool. Mirrors the admin update path: setting `address` re-queues the
/// geocoder; explicit `lat`+`lng` win over an address change and set status
/// to `Ready`; the embedding is refreshed best-effort.
pub async fn apply_facility_patch(
    state: &AppState,
    id: Uuid,
    body: Value,
) -> Result<FacilityRecord, AppError> {
    let parsed: PatchFacilityBody = serde_json::from_value(body)
        .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))?;

    let coords = validate_coords(parsed.lat, parsed.lng)?;
    let address_changed = parsed.address.is_some();

    let mut updated = state.db.update_facility_metadata(
        id,
        parsed.name,
        parsed.address,
        parsed.contacts,
        parsed.notes,
        parsed.tags,
        parsed.blob_ids,
    ).await?;

    // Best-effort embedding refresh — non-fatal if Ollama is unreachable.
    let embed_input = updated.embedding_text();
    if let Ok(embedding) = embed_text(&state.ai, &embed_input).await {
        let _ = state.db.update_facility_embedding(id, embedding).await;
    }

    if let Some((lat, lng)) = coords {
        updated = state.db.set_facility_coords_manual(id, lat, lng).await?;
    } else if address_changed {
        let _ = state.geocoding_tx.try_send(id);
    }

    Ok(updated)
}

// --- Delete tiers (soft archive / reactivate / permanent) ---

#[utoipa::path(
    delete,
    path = "/fleet/api/v1/facilities/{id}",
    params(("id" = Uuid, Path, description = "Facility UUID")),
    responses(
        (status = 204, description = "Facility archived (soft delete)"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Facility not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
/// Tier 1 — the everyday "Delete": a reversible soft archive (`facilities:write`).
/// The row persists as a reference target; it just drops out of active lists and
/// the stop typeahead. No referrer check needed (it's always safe).
pub async fn archive_facility_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("facilities:write")?;
    state.db.set_facility_archived(id, true).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/facilities/{id}/reactivate",
    params(("id" = Uuid, Path, description = "Facility UUID")),
    responses(
        (status = 200, description = "Reactivated facility record", body = FacilityRecord),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Facility not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
/// Reverse of the soft archive (`facilities:write`).
pub async fn reactivate_facility_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("facilities:write")?;
    let record = state.db.set_facility_archived(id, false).await?;
    Ok(Json(record))
}

#[utoipa::path(
    delete,
    path = "/fleet/api/v1/facilities/{id}/permanent",
    params(("id" = Uuid, Path, description = "Facility UUID")),
    responses(
        (status = 204, description = "Facility permanently deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Facility not found"),
        (status = 409, description = "Conflict — referenced by load or trip stops"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
/// Tier 2 — the deliberately difficult permanent purge (`facilities:delete`).
/// Refused with 409 + an enumerated referrer list when any load or trip stop
/// references the facility; the user must clear the referrers first.
pub async fn permanent_delete_facility_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("facilities:delete")?;
    // 404 early if it doesn't exist.
    state.db.get_facility_by_id(id).await?;
    let loads = state.db.count_loads_referencing_facility(id).await?;
    let trips = state.db.count_trips_referencing_facility(id).await?;
    if loads + trips > 0 {
        return Err(AppError::Conflict(referrer_conflict_message(
            "facility",
            &[("loads", loads), ("trips", trips)],
        )));
    }
    state.db.delete_facility_by_id(id).await?;
    Ok(StatusCode::NO_CONTENT)
}
