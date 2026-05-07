use crate::{
    error::AppError,
    events,
    models::{DriverStatus, LoadStatus, TrailerStatus, TripStatus, TruckStatus},
    models::trip::TripStopType,
    AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, ToSchema)]
pub struct AssignTripRequest {
    pub driver_id: Uuid,
    pub truck_id: Uuid,
    #[serde(default)]
    pub trailer_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct StopArriveRequest {
    pub actual_arrive: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct StopDepartRequest {
    pub actual_depart: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct StopLateRequest {
    pub eta: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CheckCallRequest {
    pub location: String,
    pub notes: Option<String>,
    pub eta_next_stop: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/trips/{id}/assign",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    request_body(content = AssignTripRequest, description = "Driver, truck, and optional trailers to assign"),
    responses(
        (status = 200, description = "Trip assigned", body = TripRecord),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — driver/truck/trailer not available or invalid status transition"),
    ),
    security(("BearerAuth" = [])),
    tag = "trips"
)]
pub async fn assign_trip(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<AssignTripRequest>,
) -> Result<impl IntoResponse, AppError> {
    let driver = state.db.get_driver_by_id(body.driver_id).await?;
    if driver.status != DriverStatus::Available {
        return Err(AppError::Conflict(format!("driver {} is not available", body.driver_id)));
    }

    let truck = state.db.get_truck_by_id(body.truck_id).await?;
    if truck.status != TruckStatus::Available {
        return Err(AppError::Conflict(format!("truck {} is not available", body.truck_id)));
    }

    for &trailer_id in &body.trailer_ids {
        let trailer = state.db.get_trailer_by_id(trailer_id).await?;
        if trailer.status != TrailerStatus::Available {
            return Err(AppError::Conflict(format!("trailer {} is not available", trailer_id)));
        }
    }

    let trip = state.db.transition_trip_status(id, TripStatus::Assigned).await?;

    state.db.update_driver_status(body.driver_id, DriverStatus::Assigned).await?;
    state.db.update_truck_status(body.truck_id, TruckStatus::Assigned).await?;
    for &trailer_id in &body.trailer_ids {
        state.db.update_trailer_status(trailer_id, TrailerStatus::Assigned).await?;
    }

    if let Some(load_id) = trip.load_id {
        if let Ok(load) = state.db.get_load_by_id(load_id).await {
            if load.status == LoadStatus::Planned {
                let _ = state.db.transition_load_status(load_id, LoadStatus::Assigned, None, None, None).await;
            }
        }
    }

    events::on_trip_assigned(&state.db, id).await;
    Ok(Json(trip))
}

#[utoipa::path(
    post,
    path = "/api/v1/trips/{id}/unassign",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    responses(
        (status = 200, description = "Trip unassigned", body = TripRecord),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — invalid status transition"),
    ),
    security(("BearerAuth" = [])),
    tag = "trips"
)]
pub async fn unassign_trip(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let existing = state.db.get_trip(id).await?;
    let trip = state.db.transition_trip_status(id, TripStatus::Planned).await?;

    if let Some(driver_id) = existing.driver_id {
        let _ = state.db.update_driver_status(driver_id, DriverStatus::Available).await;
    }
    if let Some(truck_id) = existing.truck_id {
        let _ = state.db.update_truck_status(truck_id, TruckStatus::Available).await;
    }
    for &trailer_id in &existing.trailer_ids {
        let _ = state.db.update_trailer_status(trailer_id, TrailerStatus::Available).await;
    }

    if let Some(load_id) = existing.load_id {
        let active = state.db.count_active_trips_for_load(load_id).await.unwrap_or(1);
        if active == 0 {
            if let Ok(load) = state.db.get_load_by_id(load_id).await {
                if load.status == LoadStatus::Assigned {
                    let _ = state.db.transition_load_status(load_id, LoadStatus::Planned, None, None, None).await;
                }
            }
        }
    }

    events::on_trip_unassigned(&state.db, id).await;
    Ok(Json(trip))
}

#[utoipa::path(
    post,
    path = "/api/v1/trips/{id}/dispatch",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    responses(
        (status = 200, description = "Trip dispatched", body = TripRecord),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — trip must be in assigned status"),
    ),
    security(("BearerAuth" = [])),
    tag = "trips"
)]
pub async fn dispatch_trip(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let existing = state.db.get_trip(id).await?;
    if existing.status != TripStatus::Assigned {
        return Err(AppError::Conflict("trip must be in assigned status to dispatch".into()));
    }

    let trip = state.db.transition_trip_status(id, TripStatus::Dispatched).await?;

    if let Some(driver_id) = existing.driver_id {
        let _ = state.db.update_driver_status(driver_id, DriverStatus::Dispatched).await;
    }
    if let Some(truck_id) = existing.truck_id {
        let _ = state.db.update_truck_status(truck_id, TruckStatus::Dispatched).await;
    }
    for &trailer_id in &existing.trailer_ids {
        let _ = state.db.update_trailer_status(trailer_id, TrailerStatus::Dispatched).await;
    }

    if let Some(load_id) = existing.load_id {
        if let Ok(load) = state.db.get_load_by_id(load_id).await {
            if load.status == LoadStatus::Assigned {
                let _ = state.db.transition_load_status(load_id, LoadStatus::Dispatched, None, None, None).await;
            }
        }
    }

    events::on_trip_dispatched(&state.db, id).await;
    Ok(Json(trip))
}

#[utoipa::path(
    post,
    path = "/api/v1/trips/{id}/undispatch",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    responses(
        (status = 200, description = "Trip undispatched back to assigned", body = TripRecord),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — trip must be in dispatched status (not in_transit or beyond)"),
    ),
    security(("BearerAuth" = [])),
    tag = "trips"
)]
pub async fn undispatch_trip(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let existing = state.db.get_trip(id).await?;
    if existing.status != TripStatus::Dispatched {
        return Err(AppError::Conflict("trip must be in dispatched status to undispatch".into()));
    }

    let trip = state.db.transition_trip_status(id, TripStatus::Assigned).await?;

    if let Some(driver_id) = existing.driver_id {
        let _ = state.db.update_driver_status(driver_id, DriverStatus::Assigned).await;
    }
    if let Some(truck_id) = existing.truck_id {
        let _ = state.db.update_truck_status(truck_id, TruckStatus::Assigned).await;
    }
    for &trailer_id in &existing.trailer_ids {
        let _ = state.db.update_trailer_status(trailer_id, TrailerStatus::Assigned).await;
    }

    if let Some(load_id) = existing.load_id {
        if let Ok(all_trips) = state.db.list_trips_for_load(load_id).await {
            let any_dispatched = all_trips.iter().any(|t| {
                t.id != id && (t.status == TripStatus::Dispatched || t.status == TripStatus::InTransit)
            });
            if !any_dispatched {
                if let Ok(load) = state.db.get_load_by_id(load_id).await {
                    if load.status == LoadStatus::Dispatched {
                        let _ = state.db.transition_load_status(load_id, LoadStatus::Assigned, None, None, None).await;
                    }
                }
            }
        }
    }

    events::on_trip_undispatched(&state.db, id).await;
    Ok(Json(trip))
}

#[utoipa::path(
    post,
    path = "/api/v1/trips/{id}/cancel",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    responses(
        (status = 200, description = "Trip cancelled", body = TripRecord),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — cannot cancel a trip that is in_transit or delivered"),
    ),
    security(("BearerAuth" = [])),
    tag = "trips"
)]
pub async fn cancel_trip(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let existing = state.db.get_trip(id).await?;
    if existing.status == TripStatus::InTransit || existing.status == TripStatus::Delivered {
        return Err(AppError::Conflict("cannot cancel a trip that is in_transit or delivered".into()));
    }

    let trip = state.db.transition_trip_status(id, TripStatus::Cancelled).await?;

    if let Some(driver_id) = existing.driver_id {
        let _ = state.db.update_driver_status(driver_id, DriverStatus::Available).await;
    }
    if let Some(truck_id) = existing.truck_id {
        let _ = state.db.update_truck_status(truck_id, TruckStatus::Available).await;
    }
    for &trailer_id in &existing.trailer_ids {
        let _ = state.db.update_trailer_status(trailer_id, TrailerStatus::Available).await;
    }

    if let Some(load_id) = existing.load_id {
        let active = state.db.count_active_trips_for_load(load_id).await.unwrap_or(1);
        if active == 0 {
            if let Ok(load) = state.db.get_load_by_id(load_id).await {
                if load.status == LoadStatus::Planned || load.status == LoadStatus::Assigned {
                    let _ = state.db.transition_load_status(load_id, LoadStatus::Planned, None, None, None).await;
                }
            }
        }
    }

    events::on_trip_cancelled(&state.db, id).await;
    Ok(Json(trip))
}

#[utoipa::path(
    post,
    path = "/api/v1/trips/{id}/stops/{seq}/arrive",
    params(
        ("id" = Uuid, Path, description = "Trip UUID"),
        ("seq" = u32, Path, description = "Stop sequence number"),
    ),
    request_body(content = StopArriveRequest, description = "Actual arrival time"),
    responses(
        (status = 200, description = "Stop arrival recorded", body = TripRecord),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "trips"
)]
pub async fn stop_arrive(
    State(state): State<AppState>,
    Path((id, seq)): Path<(Uuid, u32)>,
    Json(body): Json<StopArriveRequest>,
) -> Result<impl IntoResponse, AppError> {
    let trip = state.db.update_trip_stop(id, seq, Some(body.actual_arrive.clone()), None).await?;

    if let Some(load_id) = trip.load_id {
        if let Some(stop) = trip.stops.iter().find(|s| s.sequence == seq) {
            if let Some(load_stop_idx) = stop.load_stop_index {
                if let Ok(load) = state.db.get_load_by_id(load_id).await {
                    let mut updated_stops = load.stops.clone();
                    if (load_stop_idx as usize) < updated_stops.len() {
                        updated_stops[load_stop_idx as usize].actual_arrive = Some(body.actual_arrive.clone());
                        let _ = state.db.update_load_metadata(
                            load_id, None, None, Some(updated_stops),
                            None, None, None, None, None, None, None, None,
                        ).await;
                    }
                }
            }
        }
    }

    events::on_stop_arrived(&state.db, id, seq).await;
    Ok(Json(trip))
}

#[utoipa::path(
    post,
    path = "/api/v1/trips/{id}/stops/{seq}/depart",
    params(
        ("id" = Uuid, Path, description = "Trip UUID"),
        ("seq" = u32, Path, description = "Stop sequence number"),
    ),
    request_body(content = StopDepartRequest, description = "Actual departure time"),
    responses(
        (status = 200, description = "Stop departure recorded", body = TripRecord),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "trips"
)]
pub async fn stop_depart(
    State(state): State<AppState>,
    Path((id, seq)): Path<(Uuid, u32)>,
    Json(body): Json<StopDepartRequest>,
) -> Result<impl IntoResponse, AppError> {
    let trip = state.db.update_trip_stop(id, seq, None, Some(body.actual_depart.clone())).await?;

    // Cascade actual_depart to load stop if linked
    if let Some(load_id) = trip.load_id {
        if let Some(stop) = trip.stops.iter().find(|s| s.sequence == seq) {
            if let Some(load_stop_idx) = stop.load_stop_index {
                if let Ok(load) = state.db.get_load_by_id(load_id).await {
                    let mut updated_stops = load.stops.clone();
                    if (load_stop_idx as usize) < updated_stops.len() {
                        updated_stops[load_stop_idx as usize].actual_depart = Some(body.actual_depart.clone());
                        let _ = state.db.update_load_metadata(
                            load_id, None, None, Some(updated_stops),
                            None, None, None, None, None, None, None, None,
                        ).await;
                    }
                }
            }

            // If this is a pickup stop and trip is dispatched → trip goes in_transit
            let is_pickup = stop.stop_type == TripStopType::Pickup;
            if is_pickup && trip.status == TripStatus::Dispatched {
                if let Ok(updated_trip) = state.db.transition_trip_status(id, TripStatus::InTransit).await {
                    // Cascade load to in_transit if load is dispatched
                    if let Ok(load) = state.db.get_load_by_id(load_id).await {
                        if load.status == LoadStatus::Dispatched {
                            let _ = state.db.transition_load_status(load_id, LoadStatus::InTransit, None, None, None).await;
                        }
                    }
                    events::on_trip_in_transit(&state.db, id).await;
                    let _ = updated_trip; // trip variable already used above
                }
            }
        }
    } else if let Some(stop) = trip.stops.iter().find(|s| s.sequence == seq) {
        // No load_id branch — still check pickup for in_transit
        let is_pickup = stop.stop_type == TripStopType::Pickup;
        if is_pickup && trip.status == TripStatus::Dispatched {
            let _ = state.db.transition_trip_status(id, TripStatus::InTransit).await;
            events::on_trip_in_transit(&state.db, id).await;
        }
    }

    // Re-fetch current trip state to check for final stop
    let current_trip = state.db.get_trip(id).await?;
    let max_seq = current_trip.stops.iter().map(|s| s.sequence).max();
    if Some(seq) == max_seq && current_trip.status == TripStatus::InTransit {
        if let Ok(delivered_trip) = state.db.transition_trip_status(id, TripStatus::Delivered).await {
            events::on_trip_delivered(&state.db, id).await;

            // Check if all trips for load are delivered → cascade load
            if let Some(load_id) = delivered_trip.load_id {
                if let Ok(all_trips) = state.db.list_trips_for_load(load_id).await {
                    let all_delivered = all_trips.iter().all(|t| t.status == TripStatus::Delivered);
                    if all_delivered {
                        if let Ok(load) = state.db.get_load_by_id(load_id).await {
                            if load.status == LoadStatus::InTransit {
                                let _ = state.db.transition_load_status(load_id, LoadStatus::Delivered, None, None, None).await;
                            }
                        }
                    }
                }
            }
        }
    }

    events::on_stop_departed(&state.db, id, seq).await;
    Ok(Json(current_trip))
}

#[utoipa::path(
    post,
    path = "/api/v1/trips/{id}/stops/{seq}/late",
    params(
        ("id" = Uuid, Path, description = "Trip UUID"),
        ("seq" = u32, Path, description = "Stop sequence number"),
    ),
    request_body(content = StopLateRequest, description = "ETA and optional notes"),
    responses(
        (status = 204, description = "Late flag recorded"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "trips"
)]
pub async fn stop_late(
    State(state): State<AppState>,
    Path((id, seq)): Path<(Uuid, u32)>,
    Json(body): Json<StopLateRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Verify trip exists
    state.db.get_trip(id).await?;
    events::on_stop_late(&state.db, id, seq, body.eta, body.notes).await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/trips/{id}/check-call",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    request_body(content = CheckCallRequest, description = "Location, notes, and optional next-stop ETA"),
    responses(
        (status = 204, description = "Check call recorded"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "trips"
)]
pub async fn check_call(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<CheckCallRequest>,
) -> Result<impl IntoResponse, AppError> {
    state.db.get_trip(id).await?;
    events::on_check_call(&state.db, id, body.location, body.notes, body.eta_next_stop).await;
    Ok(StatusCode::NO_CONTENT)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/trips/:id/assign", post(assign_trip))
        .route("/api/v1/trips/:id/unassign", post(unassign_trip))
        .route("/api/v1/trips/:id/dispatch", post(dispatch_trip))
        .route("/api/v1/trips/:id/undispatch", post(undispatch_trip))
        .route("/api/v1/trips/:id/cancel", post(cancel_trip))
        .route("/api/v1/trips/:id/stops/:seq/arrive", post(stop_arrive))
        .route("/api/v1/trips/:id/stops/:seq/depart", post(stop_depart))
        .route("/api/v1/trips/:id/stops/:seq/late", post(stop_late))
        .route("/api/v1/trips/:id/check-call", post(check_call))
}
