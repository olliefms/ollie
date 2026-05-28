// src/api/driver_actions.rs
//
// Dispatcher/fleet-manager–facing equipment management. Unlike the driver
// portal (which lets a driver swap their own trailers but never trucks), these
// endpoints let a dispatcher seat/un-seat a driver on a truck and add/drop
// trailers — including un-seating a driver who is being dismissed so their
// equipment is freed. These are equipment events only; they never transition
// trip status.
use crate::{
    error::AppError,
    events,
    models::{DriverStatus, TrailerStatus, TripStatus, TruckStatus},
    AppState,
};
use axum::{
    extract::{Path, State},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use std::collections::HashSet;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, ToSchema)]
pub struct AttachEquipmentRequest {
    /// Truck to seat the driver on. If the driver is already on a truck, the
    /// previous one is released to `Available` first.
    pub truck_id: Option<Uuid>,
    /// Trailers to add to the driver (additive — existing trailers are kept).
    #[serde(default)]
    pub trailer_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DetachEquipmentRequest {
    /// Un-seat the driver from their current truck (release to `Available`).
    #[serde(default)]
    pub truck: bool,
    /// Specific trailers to detach. Ignored when `all_trailers` is true.
    #[serde(default)]
    pub trailer_ids: Vec<Uuid>,
    /// Detach every trailer currently attached to the driver.
    #[serde(default)]
    pub all_trailers: bool,
}

/// The driver's active trip, if one is currently rolling.
async fn active_trip_for_driver(
    state: &AppState,
    driver_id: Uuid,
) -> Option<crate::models::trip::TripListItem> {
    let trips = state.db.list_trips(None, Some(driver_id), None).await.ok()?;
    trips
        .into_iter()
        .find(|t| matches!(t.status, TripStatus::Dispatched | TripStatus::InTransit))
}

/// Reject equipment that is currently on another driver's active (dispatched or
/// in-transit) trip — it cannot be double-booked while rolling.
async fn reject_equipment_in_use(
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
        if let Some(tk) = truck_id {
            if t.truck_id == Some(tk) {
                return Err(AppError::Conflict(format!(
                    "truck {tk} is on another driver's active trip"
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

#[utoipa::path(
    post,
    path = "/api/v1/drivers/{id}/attach-equipment",
    params(("id" = Uuid, Path, description = "Driver UUID")),
    request_body(content = AttachEquipmentRequest, description = "Truck and/or trailers to attach to the driver"),
    responses(
        (status = 200, description = "Equipment attached", body = DriverRecord),
        (status = 400, description = "Bad request — no equipment specified"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Driver, truck, or trailer not found"),
        (status = 409, description = "Conflict — driver inactive, equipment out of service, or already on another active trip"),
    ),
    security(("BearerAuth" = [])),
    tag = "drivers"
)]
pub async fn attach_equipment(
    State(state): State<AppState>,
    Path(driver_id): Path<Uuid>,
    Json(body): Json<AttachEquipmentRequest>,
) -> Result<impl IntoResponse, AppError> {
    let driver = state.db.get_driver_by_id(driver_id).await?;
    if driver.status == DriverStatus::Inactive {
        return Err(AppError::Conflict(format!(
            "driver {driver_id} is inactive"
        )));
    }
    if body.truck_id.is_none() && body.trailer_ids.is_empty() {
        return Err(AppError::BadRequest(
            "must supply truck_id and/or trailer_ids".into(),
        ));
    }

    let on_active_trip = active_trip_for_driver(&state, driver_id).await;
    let truck_target = if on_active_trip.is_some() {
        TruckStatus::Dispatched
    } else {
        TruckStatus::Assigned
    };
    let trailer_target = if on_active_trip.is_some() {
        TrailerStatus::Dispatched
    } else {
        TrailerStatus::Assigned
    };

    // ── Pre-validate everything before mutating, to avoid partial state ──
    if let Some(truck_id) = body.truck_id {
        let truck = state.db.get_truck_by_id(truck_id).await?;
        if matches!(truck.status, TruckStatus::OutOfService | TruckStatus::Inactive) {
            return Err(AppError::Conflict(format!(
                "truck {truck_id} is not available for attachment"
            )));
        }
    }

    let existing_trailers: HashSet<Uuid> = driver.current_trailer_ids.iter().copied().collect();
    let mut new_trailer_ids: Vec<Uuid> = Vec::new();
    let mut seen = HashSet::new();
    for &tid in &body.trailer_ids {
        if !seen.insert(tid) {
            return Err(AppError::BadRequest(
                "duplicate trailer in request".into(),
            ));
        }
        if existing_trailers.contains(&tid) {
            continue; // already attached — no-op
        }
        let trailer = state.db.get_trailer_by_id(tid).await?;
        if matches!(trailer.status, TrailerStatus::OutOfService | TrailerStatus::Inactive) {
            return Err(AppError::Conflict(format!(
                "trailer {tid} is not available for attachment"
            )));
        }
        new_trailer_ids.push(tid);
    }

    reject_equipment_in_use(&state, driver_id, body.truck_id, &new_trailer_ids).await?;

    // ── Mutate ──
    let mut current_truck = driver.current_truck_id;
    if let Some(truck_id) = body.truck_id {
        if current_truck != Some(truck_id) {
            if let Some(prev) = current_truck {
                let _ = state.db.update_truck_status(prev, TruckStatus::Available).await;
            }
            current_truck = Some(truck_id);
        }
        state.db.update_truck_status(truck_id, truck_target).await?;
    }

    let mut trailers = driver.current_trailer_ids.clone();
    for &tid in &new_trailer_ids {
        state.db.update_trailer_status(tid, trailer_target.clone()).await?;
        trailers.push(tid);
    }

    let updated = state
        .db
        .update_driver_equipment(driver_id, Some(current_truck), Some(trailers.clone()))
        .await?;

    let mut trip_cascade = false;
    let trip_id = on_active_trip.as_ref().map(|t| t.id);
    if let Some(trip) = &on_active_trip {
        state
            .db
            .update_trip_resources(trip.id, trip.driver_id, current_truck, trailers.clone())
            .await?;
        trip_cascade = true;
    }

    events::on_driver_equipment_changed(
        &state.db,
        driver_id,
        "attach",
        current_truck,
        &trailers,
        trip_id,
        trip_cascade,
    )
    .await;

    Ok(Json(updated))
}

#[utoipa::path(
    post,
    path = "/api/v1/drivers/{id}/detach-equipment",
    params(("id" = Uuid, Path, description = "Driver UUID")),
    request_body(content = DetachEquipmentRequest, description = "Truck and/or trailers to detach from the driver"),
    responses(
        (status = 200, description = "Equipment detached", body = DriverRecord),
        (status = 400, description = "Bad request — nothing to detach"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Driver not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "drivers"
)]
pub async fn detach_equipment(
    State(state): State<AppState>,
    Path(driver_id): Path<Uuid>,
    Json(body): Json<DetachEquipmentRequest>,
) -> Result<impl IntoResponse, AppError> {
    let driver = state.db.get_driver_by_id(driver_id).await?;
    if !body.truck && !body.all_trailers && body.trailer_ids.is_empty() {
        return Err(AppError::BadRequest(
            "must specify truck, trailer_ids, or all_trailers".into(),
        ));
    }

    let on_active_trip = active_trip_for_driver(&state, driver_id).await;

    // ── Truck ──
    let mut current_truck = driver.current_truck_id;
    if body.truck {
        if let Some(prev) = current_truck {
            let _ = state.db.update_truck_status(prev, TruckStatus::Available).await;
        }
        current_truck = None;
    }

    // ── Trailers ──
    let to_remove: HashSet<Uuid> = if body.all_trailers {
        driver.current_trailer_ids.iter().copied().collect()
    } else {
        let requested: HashSet<Uuid> = body.trailer_ids.iter().copied().collect();
        driver
            .current_trailer_ids
            .iter()
            .copied()
            .filter(|tid| requested.contains(tid))
            .collect()
    };
    for tid in &to_remove {
        let _ = state.db.update_trailer_status(*tid, TrailerStatus::Available).await;
    }
    let trailers: Vec<Uuid> = driver
        .current_trailer_ids
        .iter()
        .copied()
        .filter(|tid| !to_remove.contains(tid))
        .collect();

    let updated = state
        .db
        .update_driver_equipment(driver_id, Some(current_truck), Some(trailers.clone()))
        .await?;

    let mut trip_cascade = false;
    let trip_id = on_active_trip.as_ref().map(|t| t.id);
    if let Some(trip) = &on_active_trip {
        state
            .db
            .update_trip_resources(trip.id, trip.driver_id, current_truck, trailers.clone())
            .await?;
        trip_cascade = true;
    }

    events::on_driver_equipment_changed(
        &state.db,
        driver_id,
        "detach",
        current_truck,
        &trailers,
        trip_id,
        trip_cascade,
    )
    .await;

    Ok(Json(updated))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/drivers/:id/attach-equipment", post(attach_equipment))
        .route("/api/v1/drivers/:id/detach-equipment", post(detach_equipment))
}
