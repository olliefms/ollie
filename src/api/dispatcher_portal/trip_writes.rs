// src/api/dispatcher_portal/trip_writes.rs
//
// Dispatcher-portal trip write endpoints (#259, #262):
//   - POST /dispatch/api/v1/trips/{id}/recalculate-miles
//   - PATCH /dispatch/api/v1/trips/{id}
//
// Both endpoints share the same auth middleware as the rest of dispatcher_portal.
// Mileage (deadhead_miles / loaded_miles / total_miles / segment_miles) is NEVER
// directly settable through these endpoints — only ORS persists those values via
// `compute_and_persist_mileage`.

use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    AppState,
    api::trips::compute_and_persist_mileage,
    error::AppError,
};

use super::data::build_trip_detail;

#[derive(Debug, Deserialize, ToSchema, Default)]
pub struct RecalculateMilesBody {
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchTripBody {
    #[serde(default)]
    pub notes: Option<String>,
    /// `Some(uuid)` sets the link; omitted = no change.
    /// Note: clearing previous_trip_id to null is not currently supported via this
    /// endpoint (would require a `double_option` serde pattern); see follow-up.
    #[serde(default)]
    pub previous_trip_id: Option<Uuid>,
}

#[utoipa::path(
    post,
    path = "/dispatch/api/v1/trips/{id}/recalculate-miles",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    request_body(content = RecalculateMilesBody, description = "Optional { force: bool }"),
    responses(
        (status = 200, description = "Updated mileage summary (or existing summary when nothing to do)"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Trip not found"),
        (status = 409, description = "ORS unavailable or facility coordinates missing"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn recalculate_miles_handler(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    body: Option<Json<RecalculateMilesBody>>,
) -> Result<impl IntoResponse, AppError> {
    let force = body.map(|Json(b)| b.force).unwrap_or(false);

    let trip = state.db.get_trip(id).await?;
    let already_set = trip.deadhead_miles.is_some() && trip.loaded_miles.is_some();

    let summary = if !force && already_set {
        crate::api::mileage_summary::build_mileage_summary(&state, &trip).await
    } else {
        compute_and_persist_mileage(&state, id).await?
    };

    Ok(Json(summary))
}

#[utoipa::path(
    patch,
    path = "/dispatch/api/v1/trips/{id}",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    request_body(content = PatchTripBody, description = "Allowed fields: notes, previous_trip_id"),
    responses(
        (status = 200, description = "Updated trip record (enriched, with mileage_summary)"),
        (status = 400, description = "Bad request — unknown or disallowed field"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Trip not found"),
        (status = 409, description = "ORS unavailable while recomputing mileage"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn patch_trip_handler(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<impl IntoResponse, AppError> {
    // Reject raw-mileage fields explicitly with a friendly message before generic
    // deny_unknown_fields kicks in.
    if let serde_json::Value::Object(map) = &body {
        for forbidden in [
            "deadhead_miles", "loaded_miles", "total_miles", "segment_miles",
        ] {
            if map.contains_key(forbidden) {
                return Err(AppError::BadRequest(format!(
                    "{forbidden} is computed by routing and cannot be set directly"
                )));
            }
        }
    }

    let parsed: PatchTripBody = serde_json::from_value(body)
        .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))?;

    // Apply notes update if present.
    if parsed.notes.is_some() {
        state.db.update_trip_metadata(
            id, None, None, None, parsed.notes.clone(), None,
        ).await?;
    }

    // Apply previous_trip_id update if present, then re-fire mileage recompute.
    if let Some(new_prev) = parsed.previous_trip_id {
        state.db
            .update_trip_previous_trip_id(id, Some(new_prev))
            .await?;
        compute_and_persist_mileage(&state, id).await?;
    }

    let item = build_trip_detail(&state, id).await?;
    Ok(Json(item))
}
