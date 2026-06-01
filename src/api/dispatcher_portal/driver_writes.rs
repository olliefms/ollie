// src/api/dispatcher_portal/driver_writes.rs
//
// Dispatcher-portal driver equipment write endpoints (#181):
//   - POST /dispatch/api/v1/drivers/{id}/attach-equipment
//   - POST /dispatch/api/v1/drivers/{id}/detach-equipment
//
// Dispatcher-facing equipment management the driver portal does not cover: a
// dispatcher can seat a driver on a truck and/or add trailers (attach), or
// un-seat the truck and drop trailers (detach), releasing them back to
// `Available`. These are pure equipment events — they never transition trip
// status. When the driver has an active (Dispatched/InTransit) trip, the
// changes cascade into `trip.truck_id` / `trip.trailer_ids` and emit a
// `driver.equipment_changed` event.
//
// The `apply_*` helpers are shared with the MCP `attach_equipment` /
// `detach_equipment` tools so HTTP and MCP enforce the same validation and
// side effects.

use std::collections::HashSet;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use super::jwt::DispatcherClaims;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    ai::embed::embed_text,
    error::AppError,
    events,
    models::{CreateDriverRequest, DriverCredentials, DriverRecord, DriverStatus, SetDriverPinRequest, TrailerStatus, TripStatus, TruckStatus, UpdateDriverRequest},
    AppState,
};

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachEquipmentBody {
    /// Truck to seat the driver on. Releases the driver's previous truck to
    /// `Available` first. Omit to leave the current truck unchanged.
    #[serde(default)]
    pub truck: Option<Uuid>,
    /// Trailers to add (additive — merged with any already attached).
    #[serde(default)]
    pub trailer_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DetachEquipmentBody {
    /// Un-seat the driver's truck, releasing it to `Available`.
    #[serde(default)]
    pub truck: bool,
    /// Specific trailers to drop. Released to `Available`.
    #[serde(default)]
    pub trailer_ids: Vec<Uuid>,
    /// Drop every attached trailer.
    #[serde(default)]
    pub all_trailers: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DriverEquipmentChange {
    pub driver_id: Uuid,
    pub truck_id: Option<Uuid>,
    pub trailer_ids: Vec<Uuid>,
    pub trip_cascade: bool,
    pub trip_id: Option<Uuid>,
}

#[utoipa::path(
    post,
    path = "/dispatch/api/v1/drivers/{id}/attach-equipment",
    params(("id" = Uuid, Path, description = "Driver UUID")),
    request_body(content = AttachEquipmentBody, description = "Truck and/or trailers to attach"),
    responses(
        (status = 200, description = "Updated driver equipment", body = DriverEquipmentChange),
        (status = 400, description = "Bad request — empty body or invalid fields"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Driver, truck, or trailer not found"),
        (status = 409, description = "Conflict — driver inactive or equipment on another active trip"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn attach_equipment_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("drivers:write")?;
    let change = apply_attach_equipment(&state, id, body).await?;
    Ok(Json(change))
}

#[utoipa::path(
    post,
    path = "/dispatch/api/v1/drivers/{id}/detach-equipment",
    params(("id" = Uuid, Path, description = "Driver UUID")),
    request_body(content = DetachEquipmentBody, description = "Truck and/or trailers to detach"),
    responses(
        (status = 200, description = "Updated driver equipment", body = DriverEquipmentChange),
        (status = 400, description = "Bad request — nothing to detach or invalid fields"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Driver not found"),
        (status = 409, description = "Conflict — driver inactive"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn detach_equipment_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("drivers:write")?;
    let change = apply_detach_equipment(&state, id, body).await?;
    Ok(Json(change))
}

/// A driver's active trip for equipment cascade — a Dispatched/InTransit trip
/// (dispatch allows at most one per driver). Most recent wins within the tier.
async fn active_trip_for_driver(
    state: &AppState,
    driver_id: Uuid,
) -> Result<Option<crate::models::TripListItem>, AppError> {
    let mut trips: Vec<_> = state
        .db
        .list_trips(None, Some(driver_id), None, None, None)
        .await?
        .into_iter()
        .filter(|t| matches!(t.status, TripStatus::Dispatched | TripStatus::InTransit))
        .collect();
    trips.sort_by_key(|t| std::cmp::Reverse(t.created_at));
    Ok(trips.into_iter().next())
}

/// Reject equipment that sits on another driver's active (Dispatched/InTransit)
/// trip. `driver_id` is the driver we are attaching to (their own trips are OK).
async fn reject_equipment_on_other_active_trip(
    state: &AppState,
    driver_id: Uuid,
    truck_id: Option<Uuid>,
    trailer_ids: &[Uuid],
) -> Result<(), AppError> {
    if truck_id.is_none() && trailer_ids.is_empty() {
        return Ok(());
    }
    let trips = state.db.list_trips(None, None, None, None, None).await?;
    for t in &trips {
        if !matches!(t.status, TripStatus::Dispatched | TripStatus::InTransit) {
            continue;
        }
        if t.driver_id == Some(driver_id) {
            continue;
        }
        if let Some(tid) = truck_id {
            if t.truck_id == Some(tid) {
                return Err(AppError::Conflict(format!(
                    "truck {tid} is on another driver's active trip"
                )));
            }
        }
        for tid in trailer_ids {
            if t.trailer_ids.contains(tid) {
                return Err(AppError::Conflict(format!(
                    "trailer {tid} is on another driver's active trip"
                )));
            }
        }
    }
    Ok(())
}

/// Shared attach writer — seats a driver on a truck and/or adds trailers
/// (additive). Used by the HTTP handler and the MCP `attach_equipment` tool.
pub async fn apply_attach_equipment(
    state: &AppState,
    driver_id: Uuid,
    body: Value,
) -> Result<DriverEquipmentChange, AppError> {
    let parsed: AttachEquipmentBody = serde_json::from_value(body)
        .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))?;

    if parsed.truck.is_none() && parsed.trailer_ids.is_empty() {
        return Err(AppError::BadRequest(
            "specify a truck and/or trailer_ids to attach".into(),
        ));
    }

    let driver = state.db.get_driver_by_id(driver_id).await?;
    if driver.status == DriverStatus::Inactive {
        return Err(AppError::Conflict("driver is inactive".into()));
    }

    // Validate requested equipment exists and is not inactive/out of service.
    if let Some(tid) = parsed.truck {
        let truck = state.db.get_truck_by_id(tid).await?;
        if matches!(truck.status, TruckStatus::Inactive | TruckStatus::OutOfService) {
            return Err(AppError::Conflict(format!(
                "truck {} is {}",
                truck.unit_number,
                truck.status.as_str()
            )));
        }
    }
    for tid in &parsed.trailer_ids {
        let trailer = state.db.get_trailer_by_id(*tid).await?;
        if matches!(
            trailer.status,
            TrailerStatus::Inactive | TrailerStatus::OutOfService
        ) {
            return Err(AppError::Conflict(format!(
                "trailer {} is {}",
                trailer.unit_number,
                trailer.status.as_str()
            )));
        }
    }

    // Reject equipment already on another driver's active trip.
    reject_equipment_on_other_active_trip(state, driver_id, parsed.truck, &parsed.trailer_ids)
        .await?;

    let active_trip = active_trip_for_driver(state, driver_id).await?;

    // Baseline equipment is the driver's recorded equipment, falling back to the
    // active trip's resources when the driver record has not been seeded yet
    // (e.g. assigned via the trip lifecycle, which sets the trip but not the
    // driver record). This keeps attach additive against what the driver is
    // actually pulling, not just what was last written to the driver record.
    let previous_truck_id = driver
        .current_truck_id
        .or_else(|| active_trip.as_ref().and_then(|t| t.truck_id));
    let previous_trailer_ids: Vec<Uuid> = if !driver.current_trailer_ids.is_empty() {
        driver.current_trailer_ids.clone()
    } else {
        active_trip
            .as_ref()
            .map(|t| t.trailer_ids.clone())
            .unwrap_or_default()
    };

    // Resolve the new truck: attaching a truck releases the previous one first.
    let new_truck_id = match parsed.truck {
        Some(tid) => {
            if let Some(prev) = previous_truck_id {
                if prev != tid {
                    let _ = state.db.update_truck_status(prev, TruckStatus::Available).await;
                }
            }
            Some(tid)
        }
        None => previous_truck_id,
    };

    // Trailers are additive: union of current + new, order-preserving.
    let mut new_trailer_ids = previous_trailer_ids.clone();
    let mut seen: HashSet<Uuid> = previous_trailer_ids.iter().copied().collect();
    for tid in &parsed.trailer_ids {
        if seen.insert(*tid) {
            new_trailer_ids.push(*tid);
        }
    }

    let attach_status_truck = match active_trip.as_ref().map(|t| &t.status) {
        Some(TripStatus::Dispatched) | Some(TripStatus::InTransit) => TruckStatus::Dispatched,
        _ => TruckStatus::Assigned,
    };
    let attach_status_trailer = match active_trip.as_ref().map(|t| &t.status) {
        Some(TripStatus::Dispatched) | Some(TripStatus::InTransit) => TrailerStatus::Dispatched,
        _ => TrailerStatus::Assigned,
    };

    // Persist the driver's equipment.
    let updated = state
        .db
        .update_driver_equipment(driver_id, Some(new_truck_id), Some(new_trailer_ids.clone()))
        .await?;

    // Mark newly attached equipment.
    if let Some(tid) = parsed.truck {
        let _ = state.db.update_truck_status(tid, attach_status_truck).await;
    }
    for tid in &parsed.trailer_ids {
        let _ = state.db.update_trailer_status(*tid, attach_status_trailer.clone()).await;
    }

    // Cascade into the active trip when present.
    let (trip_cascade, trip_id) = sync_active_trip(
        state,
        &active_trip,
        updated.current_truck_id,
        &updated.current_trailer_ids,
    )
    .await?;

    events::on_driver_equipment_changed(
        &state.db,
        driver_id,
        previous_truck_id,
        updated.current_truck_id,
        &previous_trailer_ids,
        &updated.current_trailer_ids,
        trip_id,
        trip_cascade,
    )
    .await;

    Ok(DriverEquipmentChange {
        driver_id,
        truck_id: updated.current_truck_id,
        trailer_ids: updated.current_trailer_ids,
        trip_cascade,
        trip_id,
    })
}

/// Shared detach writer — un-seats a truck and/or drops trailers, releasing
/// them to `Available`. Used by the HTTP handler and the MCP `detach_equipment`
/// tool.
pub async fn apply_detach_equipment(
    state: &AppState,
    driver_id: Uuid,
    body: Value,
) -> Result<DriverEquipmentChange, AppError> {
    let parsed: DetachEquipmentBody = serde_json::from_value(body)
        .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))?;

    if !parsed.truck && !parsed.all_trailers && parsed.trailer_ids.is_empty() {
        return Err(AppError::BadRequest(
            "specify truck, trailer_ids, or all_trailers to detach".into(),
        ));
    }

    let driver = state.db.get_driver_by_id(driver_id).await?;
    if driver.status == DriverStatus::Inactive {
        return Err(AppError::Conflict("driver is inactive".into()));
    }

    let active_trip = active_trip_for_driver(state, driver_id).await?;

    // Baseline equipment falls back to the active trip when the driver record
    // has not been seeded (assigned via the trip lifecycle).
    let previous_truck_id = driver
        .current_truck_id
        .or_else(|| active_trip.as_ref().and_then(|t| t.truck_id));
    let previous_trailer_ids: Vec<Uuid> = if !driver.current_trailer_ids.is_empty() {
        driver.current_trailer_ids.clone()
    } else {
        active_trip
            .as_ref()
            .map(|t| t.trailer_ids.clone())
            .unwrap_or_default()
    };

    // Resolve trailers to drop.
    let drop_set: HashSet<Uuid> = if parsed.all_trailers {
        previous_trailer_ids.iter().copied().collect()
    } else {
        parsed.trailer_ids.iter().copied().collect()
    };

    let new_truck_id = if parsed.truck { None } else { previous_truck_id };
    let new_trailer_ids: Vec<Uuid> = previous_trailer_ids
        .iter()
        .copied()
        .filter(|tid| !drop_set.contains(tid))
        .collect();

    let updated = state
        .db
        .update_driver_equipment(driver_id, Some(new_truck_id), Some(new_trailer_ids.clone()))
        .await?;

    // Release detached equipment to Available.
    if parsed.truck {
        if let Some(prev) = previous_truck_id {
            let _ = state.db.update_truck_status(prev, TruckStatus::Available).await;
        }
    }
    for tid in &drop_set {
        let _ = state.db.update_trailer_status(*tid, TrailerStatus::Available).await;
    }
    let (trip_cascade, trip_id) = sync_active_trip(
        state,
        &active_trip,
        updated.current_truck_id,
        &updated.current_trailer_ids,
    )
    .await?;

    events::on_driver_equipment_changed(
        &state.db,
        driver_id,
        previous_truck_id,
        updated.current_truck_id,
        &previous_trailer_ids,
        &updated.current_trailer_ids,
        trip_id,
        trip_cascade,
    )
    .await;

    Ok(DriverEquipmentChange {
        driver_id,
        truck_id: updated.current_truck_id,
        trailer_ids: updated.current_trailer_ids,
        trip_cascade,
        trip_id,
    })
}

// --- Driver create / patch (dispatcher portal) ---

/// Shared driver-create writer. Defaults terminal_id to the default terminal
/// when the request omits it. Used by the HTTP handler (and optionally MCP).
pub async fn apply_driver_create(
    state: &AppState,
    req: CreateDriverRequest,
) -> Result<DriverRecord, AppError> {
    let now = Utc::now();

    let terminal_id = match req.terminal_id {
        Some(tid) => {
            // Validate the terminal exists.
            state.db.get_terminal_by_id(tid).await?;
            Some(tid)
        }
        None => state.db.default_terminal().await.ok().map(|t| t.id),
    };

    let record = DriverRecord {
        id: Uuid::new_v4(),
        name: req.name,
        phone: req.phone,
        email: req.email,
        license_number: req.license_number,
        license_state: req.license_state,
        license_expiry: req.license_expiry,
        status: DriverStatus::Available,
        notes: req.notes,
        current_truck_id: None,
        current_trailer_ids: vec![],
        blob_ids: req.blob_ids,
        embedding: None,
        owner_id: 0,
        created_at: now,
        updated_at: now,
        terminal_id,
        loaded_rate_per_mile: req.loaded_rate_per_mile,
        deadhead_rate_per_mile: req.deadhead_rate_per_mile,
        extra_stop_fee: req.extra_stop_fee,
        detention_rate_per_hour: req.detention_rate_per_hour,
        free_dwell_minutes: req.free_dwell_minutes,
    };

    let embedding = embed_text(&state.ai, &record.embedding_text()).await.ok();
    let record = DriverRecord { embedding, ..record };
    state.db.insert_driver(&record).await?;
    Ok(record)
}

/// Shared driver-patch writer. Validates terminal exists when terminal_id is
/// supplied. Used by the HTTP handler (and optionally MCP).
pub async fn apply_driver_patch(
    state: &AppState,
    id: Uuid,
    req: UpdateDriverRequest,
) -> Result<DriverRecord, AppError> {
    // Validate terminal exists if one is being set.
    if let Some(tid) = req.terminal_id {
        state.db.get_terminal_by_id(tid).await?;
    }

    let phone = req.phone.as_deref().map(|p| {
        let stripped: String = p.chars().filter(|c| !matches!(c, ' ' | '-' | '(' | ')')).collect();
        if stripped.starts_with('+') { return stripped; }
        if stripped.len() == 10 && stripped.chars().all(|c| c.is_ascii_digit()) { return format!("+1{stripped}"); }
        if stripped.chars().all(|c| c.is_ascii_digit()) { return format!("+{stripped}"); }
        stripped
    });

    let mut updated = state.db.update_driver_metadata(
        id,
        req.name,
        phone,
        req.email,
        req.license_number,
        req.license_state,
        req.license_expiry,
        req.notes,
        req.blob_ids,
    ).await?;

    let rate_changed = req.terminal_id.is_some()
        || req.loaded_rate_per_mile.is_some()
        || req.deadhead_rate_per_mile.is_some()
        || req.extra_stop_fee.is_some()
        || req.detention_rate_per_hour.is_some()
        || req.free_dwell_minutes.is_some();

    if rate_changed {
        updated = state.db.update_driver_rate_overrides(
            id,
            req.terminal_id,
            req.loaded_rate_per_mile,
            req.deadhead_rate_per_mile,
            req.extra_stop_fee,
            req.detention_rate_per_hour,
            req.free_dwell_minutes,
        ).await?;
    }

    if let Ok(embedding) = embed_text(&state.ai, &updated.embedding_text()).await {
        let _ = state.db.update_driver_embedding(id, embedding).await;
    }

    Ok(updated)
}

#[utoipa::path(
    post,
    path = "/dispatch/api/v1/drivers",
    request_body(content = CreateDriverRequest, description = "Driver to create"),
    responses(
        (status = 201, description = "Created driver record", body = DriverRecord),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Terminal not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn create_driver_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
    Json(body): Json<CreateDriverRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("drivers:write")?;
    let record = apply_driver_create(&state, body).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(
    patch,
    path = "/dispatch/api/v1/drivers/{id}",
    params(("id" = Uuid, Path, description = "Driver UUID")),
    request_body(content = UpdateDriverRequest, description = "Fields to update"),
    responses(
        (status = 200, description = "Updated driver record", body = DriverRecord),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Driver or terminal not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn patch_driver_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateDriverRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("drivers:write")?;
    let record = apply_driver_patch(&state, id, body).await?;
    Ok(Json(record))
}

#[utoipa::path(
    delete,
    path = "/dispatch/api/v1/drivers/{id}",
    params(("id" = Uuid, Path, description = "Driver UUID")),
    responses(
        (status = 204, description = "Soft-deleted (status set to inactive); outstanding JWTs invalidated"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Driver not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn delete_driver_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("drivers:delete")?;
    apply_driver_delete(&state, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Shared driver-delete writer — used by the HTTP handler and the MCP
/// `delete_driver` tool. Soft-deletes the driver, then invalidates any
/// outstanding JWTs by bumping the credential `token_version` (propagating the
/// upsert error rather than swallowing it).
pub(crate) async fn apply_driver_delete(state: &AppState, id: Uuid) -> Result<(), AppError> {
    state.db.soft_delete_driver(id).await?;
    // Invalidate any outstanding JWTs by bumping the credential token_version.
    if let Some(mut creds) = state.db.get_driver_credentials(id).await? {
        creds.token_version += 1;
        creds.updated_at = Utc::now();
        state.db.upsert_driver_credentials(&creds).await?;
    }
    Ok(())
}

#[utoipa::path(
    post,
    path = "/dispatch/api/v1/drivers/{id}/pin",
    params(("id" = Uuid, Path, description = "Driver UUID")),
    request_body(content = SetDriverPinRequest, description = "PIN (4–6 numeric digits)"),
    responses(
        (status = 204, description = "PIN set successfully; outstanding JWTs invalidated"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Driver not found"),
        (status = 422, description = "Invalid PIN format"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn set_driver_pin_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetDriverPinRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("drivers:write")?;
    apply_set_driver_pin(&state, id, body.pin).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Shared set-PIN writer — used by the HTTP handler and the MCP `set_driver_pin`
/// tool. Validates the PIN (4–6 numeric digits), bcrypt-hashes at cost 12, and
/// upserts the driver's credentials while bumping token_version to invalidate any
/// outstanding JWTs.
pub async fn apply_set_driver_pin(
    state: &AppState,
    id: Uuid,
    pin: String,
) -> Result<(), AppError> {
    state.db.get_driver_by_id(id).await?;

    let pin = pin.trim().to_string();
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
        Some(existing) => DriverCredentials {
            driver_id: id,
            pin_hash: Some(pin_hash),
            token_version: existing.token_version + 1,
            failed_pin_attempts: 0,
            locked_until: None,
            updated_at: now,
        },
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
    Ok(())
}

/// Sync the driver's active trip's resources to the driver's current
/// equipment. Returns `(cascaded, trip_id)`.
async fn sync_active_trip(
    state: &AppState,
    active_trip: &Option<crate::models::TripListItem>,
    truck_id: Option<Uuid>,
    trailer_ids: &[Uuid],
) -> Result<(bool, Option<Uuid>), AppError> {
    let Some(trip) = active_trip else {
        return Ok((false, None));
    };
    state
        .db
        .update_trip_resources(trip.id, trip.driver_id, truck_id, trailer_ids.to_vec())
        .await?;
    Ok((true, Some(trip.id)))
}
