// src/api/fleet_portal/trip_writes.rs
//
// Fleet-portal trip write endpoints (#259, #262):
//   - POST /fleet/api/v1/trips/{id}/recalculate-miles
//   - PATCH /fleet/api/v1/trips/{id}
//
// Both endpoints share the same auth middleware as the rest of fleet_portal.
// Mileage (deadhead_miles / loaded_miles / total_miles / segment_miles) is NEVER
// directly settable through these endpoints — only ORS persists those values via
// `compute_and_persist_mileage`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use super::jwt::FleetUserClaims;
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    AppState,
    api::trips::compute_and_persist_mileage,
    error::AppError,
    models::double_option,
};

use super::data::{build_trip_detail, FleetTripListItem};

/// Result of applying a trip patch — the up-to-date detail plus an optional
/// warning when a side-effect (e.g. mileage recompute) failed *after* the
/// primary write committed. Letting the recompute fail loudly while the
/// chain link commits silently was the v1.17.0 footgun this resolves.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct PatchTripResult {
    #[serde(flatten)]
    pub detail: FleetTripListItem,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mileage_recompute_warning: Option<String>,
}

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
    /// Document blobs to attach to this trip (BOLs, PODs, lumper receipts, scale
    /// tickets). Replaces the trip's `blob_ids` when present; omitted = no change.
    #[serde(default)]
    pub blob_ids: Option<Vec<Uuid>>,
    /// Settlement reference. Setting this `None -> Some` freezes the trip's
    /// driver pay (captures a `driver_pay_snapshot`) and locks pay-affecting edits.
    #[serde(default)]
    pub settlement_ref: Option<String>,
    #[serde(default)]
    pub pay_period_start: Option<String>,
    #[serde(default)]
    pub pay_period_end: Option<String>,
    // Trip-level rate overrides (frozen once settled). `double_option` lets a
    // PATCH distinguish an omitted field (no change) from an explicit JSON `null`
    // (clear the override back to inherited / `None`) from a value (set).
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<f64>)]
    pub loaded_rate_per_mile: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<f64>)]
    pub deadhead_rate_per_mile: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<f64>)]
    pub extra_stop_fee: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<f64>)]
    pub detention_rate_per_hour: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<u32>)]
    pub free_dwell_minutes: Option<Option<u32>>,
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/trips/{id}/recalculate-miles",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    request_body(content = RecalculateMilesBody, description = "Optional { force: bool }"),
    responses(
        (status = 200, description = "Updated mileage summary (or existing summary when nothing to do)"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Trip not found"),
        (status = 409, description = "ORS unavailable or facility coordinates missing"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn recalculate_miles_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
    body: Option<Json<RecalculateMilesBody>>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trips:write")?;
    let force = body.map(|Json(b)| b.force).unwrap_or(false);

    let trip = state.db.get_trip(id).await?;
    // Settlement freeze: mileage feeds driver pay, so a settled trip's miles are frozen.
    if trip.settlement_ref.is_some() {
        return Err(AppError::Conflict(
            "trip is settled; miles are frozen".into()));
    }
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
    path = "/fleet/api/v1/trips/{id}",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    request_body(content = PatchTripBody, description = "Allowed fields: notes, previous_trip_id"),
    responses(
        (status = 200, description = "Updated trip record (enriched, with mileage_summary); \
            on partial success a `mileage_recompute_warning` field is populated"),
        (status = 400, description = "Bad request — unknown or disallowed field"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Trip not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn patch_trip_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trips:write")?;
    let result = apply_trip_patch(&state, id, body).await?;
    Ok(Json(result))
}

/// Shared write helper used by the HTTP handler and the MCP `update_trip` tool.
/// Commits `notes` and `previous_trip_id` first; mileage recompute is *best-effort*
/// — if it fails (ORS down, facility missing coords) the primary writes are still
/// persisted and the failure is reported as `mileage_recompute_warning` in the
/// returned `PatchTripResult`. Callers that need the recompute to succeed should
/// either retry once routing is healthy or call `recalculate_trip_miles` directly.
pub async fn apply_trip_patch(
    state: &AppState,
    id: Uuid,
    body: serde_json::Value,
) -> Result<PatchTripResult, AppError> {
    // Reject raw-mileage fields explicitly before generic deny_unknown_fields kicks in.
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

    // A settlement_ref, if present, must be a non-empty reference: an empty string
    // would otherwise irreversibly freeze the trip with no meaningful reference
    // (and the re-settle guard below would then make it unrecoverable).
    if parsed.settlement_ref.as_deref().is_some_and(|s| s.trim().is_empty()) {
        return Err(AppError::UnprocessableEntity(
            "settlement_ref cannot be empty".into()));
    }

    // Settlement freeze + edit-lock.
    let existing = state.db.get_trip(id).await?;
    let was_settled = existing.settlement_ref.is_some();
    let touches_rate = parsed.loaded_rate_per_mile.is_some()
        || parsed.deadhead_rate_per_mile.is_some()
        || parsed.extra_stop_fee.is_some()
        || parsed.detention_rate_per_hour.is_some()
        || parsed.free_dwell_minutes.is_some();
    if was_settled && touches_rate {
        return Err(AppError::Conflict(
            "trip is settled; pay-affecting fields are frozen".into()));
    }
    // A previous_trip_id change triggers a mileage recompute, which feeds pay.
    if was_settled && parsed.previous_trip_id.is_some() {
        return Err(AppError::Conflict(
            "trip is settled; previous_trip_id is frozen (it would recompute miles)".into()));
    }
    // Re-settling is not supported: the freeze branch below requires !was_settled,
    // so a settlement_ref change on an already-settled trip would be silently
    // dropped. Reject it explicitly, consistent with the locks above. (Pay-period
    // metadata updates remain allowed — they don't re-freeze or un-settle.)
    if was_settled && parsed.settlement_ref.is_some() {
        return Err(AppError::Conflict(
            "trip is already settled; settlement_ref cannot be changed".into()));
    }

    if parsed.notes.is_some() || parsed.blob_ids.is_some() {
        state.db.update_trip_metadata(
            id, None, None, None, parsed.notes.clone(), None, parsed.blob_ids.clone(),
        ).await?;
    }

    let mut mileage_recompute_warning: Option<String> = None;
    if let Some(new_prev) = parsed.previous_trip_id {
        state.db
            .update_trip_previous_trip_id(id, Some(new_prev))
            .await?;
        // Recompute is best-effort: the chain link is already committed and is
        // valuable on its own. Don't propagate routing failure as a hard error.
        if let Err(e) = compute_and_persist_mileage(state, id).await {
            tracing::warn!("mileage recompute after previous_trip_id update failed: {e}");
            mileage_recompute_warning = Some(e.to_string());
        }
    }

    // Apply trip rate overrides (allowed only when not settled; guarded above).
    if touches_rate {
        state.db.update_trip_rate_overrides(
            id,
            parsed.loaded_rate_per_mile,
            parsed.deadhead_rate_per_mile,
            parsed.extra_stop_fee,
            parsed.detention_rate_per_hour,
            parsed.free_dwell_minutes,
        ).await?;
    }

    // Settlement transition None -> Some: compute the snapshot from the LIVE record
    // (which now includes any rate overrides written just above), then persist it.
    if !was_settled && parsed.settlement_ref.is_some() {
        let live = state.db.get_trip(id).await?;
        let snapshot =
            crate::api::fleet_portal::data::driver_pay_for_record(state, &live).await;
        state.db.update_trip_settlement(
            id,
            parsed.settlement_ref.clone(),
            parsed.pay_period_start.clone(),
            parsed.pay_period_end.clone(),
            snapshot,
        ).await?;
    } else if parsed.pay_period_start.is_some() || parsed.pay_period_end.is_some() {
        // Updating pay-period metadata is allowed; it does not re-freeze or un-settle.
        state.db.update_trip_settlement(
            id,
            None,
            parsed.pay_period_start.clone(),
            parsed.pay_period_end.clone(),
            None,
        ).await?;
    }

    let detail = build_trip_detail(state, id).await?;
    Ok(PatchTripResult { detail, mileage_recompute_warning })
}

#[utoipa::path(
    delete,
    path = "/fleet/api/v1/trips/{id}",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    responses(
        (status = 204, description = "Deleted (soft-cancelled if active; hard-deleted if already cancelled)"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Trip not found"),
        (status = 409, description = "Cannot delete a trip that is in_transit, delivered, or completed"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn delete_trip_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trips:delete")?;
    state.db.delete_trip(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::PatchTripBody;

    // An omitted rate field deserializes to outer `None` ("no change").
    #[test]
    fn omitted_rate_field_is_outer_none() {
        let body: PatchTripBody = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(body.loaded_rate_per_mile, None);
        assert_eq!(body.free_dwell_minutes, None);
    }

    // An explicit JSON null deserializes to `Some(None)` ("clear to inherited").
    #[test]
    fn null_rate_field_is_some_none() {
        let body: PatchTripBody =
            serde_json::from_value(serde_json::json!({ "loaded_rate_per_mile": null })).unwrap();
        assert_eq!(body.loaded_rate_per_mile, Some(None));
    }

    // A value deserializes to `Some(Some(v))` ("set the override").
    #[test]
    fn value_rate_field_is_some_some() {
        let body: PatchTripBody =
            serde_json::from_value(serde_json::json!({ "loaded_rate_per_mile": 2.5 })).unwrap();
        assert_eq!(body.loaded_rate_per_mile, Some(Some(2.5)));
    }
}
