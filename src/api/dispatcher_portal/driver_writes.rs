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
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    error::AppError,
    events,
    models::{DriverStatus, TrailerStatus, TripStatus, TruckStatus},
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
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, AppError> {
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
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, AppError> {
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
        .list_trips(None, Some(driver_id), None)
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
    let trips = state.db.list_trips(None, None, None).await?;
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
