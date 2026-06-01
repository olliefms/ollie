//! Admin `/api/v1` trip-lifecycle handlers.
//!
//! These are thin axum wrappers over [`crate::services::trip_lifecycle`] (and
//! [`crate::services::trip_stops`] for stop arrive/depart). The business logic
//! lives in the service modules so the admin REST, dispatcher REST, and
//! dispatcher MCP surfaces stay behaviorally identical. This file is slated for
//! removal alongside the rest of the deprecated admin API (#236).

use crate::{error::AppError, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use uuid::Uuid;

pub use crate::services::trip_lifecycle::{
    AssignTripRequest, CheckCallRequest, StopArriveRequest, StopDepartRequest, StopLateRequest,
};

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
        (status = 409, description = "Conflict — driver/truck/trailer not eligible for assignment (inactive/out-of-service) or invalid status transition"),
    ),
    security(("BearerAuth" = [])),
    tag = "trips"
)]
pub async fn assign_trip(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<AssignTripRequest>,
) -> Result<impl IntoResponse, AppError> {
    let trip = crate::services::trip_lifecycle::assign(&state, id, body).await?;
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
    let trip = crate::services::trip_lifecycle::unassign(&state, id).await?;
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
    let trip = crate::services::trip_lifecycle::dispatch(&state, id).await?;
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
    let trip = crate::services::trip_lifecycle::undispatch(&state, id).await?;
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
    let trip = crate::services::trip_lifecycle::cancel(&state, id).await?;
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
    let existing = state.db.get_trip(id).await?;
    // Settlement freeze: a settled trip's stop times feed detention pay; they are frozen.
    if existing.settlement_ref.is_some() {
        return Err(AppError::Conflict("trip is settled; stop times are frozen".into()));
    }
    let stop_tz = existing.stops.iter()
        .find(|s| s.sequence == seq)
        .ok_or(AppError::NotFound)?
        .timezone
        .clone();
    // TOCTOU: timezone validated against pre-fetched stop; a concurrent admin change
    // is theoretically possible but negligible in practice — see #95.
    crate::services::trip_stops::validate_arrive(&body.actual_arrive, stop_tz.as_deref())?;
    let mut trip = crate::services::trip_stops::record_stop_arrive(
        &state, id, seq, body.actual_arrive,
    ).await?;
    for s in &mut trip.stops { s.fill_utc_fields(); }
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
    let existing = state.db.get_trip(id).await?;
    // Settlement freeze: a settled trip's stop times feed detention pay; they are frozen.
    if existing.settlement_ref.is_some() {
        return Err(AppError::Conflict("trip is settled; stop times are frozen".into()));
    }
    let stop_tz = existing.stops.iter()
        .find(|s| s.sequence == seq)
        .ok_or(AppError::NotFound)?
        .timezone
        .clone();
    // TOCTOU: timezone validated against pre-fetched stop; a concurrent admin change
    // is theoretically possible but negligible in practice — see #95.
    crate::services::trip_stops::validate_depart(&body.actual_depart, stop_tz.as_deref())?;
    let mut trip = crate::services::trip_stops::record_stop_depart(
        &state, id, seq, body.actual_depart,
    ).await?;
    for s in &mut trip.stops { s.fill_utc_fields(); }
    Ok(Json(trip))
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
    crate::services::trip_lifecycle::stop_late(&state, id, seq, body).await?;
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
    crate::services::trip_lifecycle::check_call(&state, id, body).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/trips/{id}/complete",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    responses(
        (status = 204, description = "Trip completed and resources released"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — trip must be in delivered status"),
    ),
    security(("BearerAuth" = [])),
    tag = "trips"
)]
pub async fn complete_trip(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    crate::services::trip_lifecycle::complete(&state, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/trips/{id}/assign", post(assign_trip))
        .route("/api/v1/trips/{id}/unassign", post(unassign_trip))
        .route("/api/v1/trips/{id}/dispatch", post(dispatch_trip))
        .route("/api/v1/trips/{id}/undispatch", post(undispatch_trip))
        .route("/api/v1/trips/{id}/cancel", post(cancel_trip))
        .route("/api/v1/trips/{id}/complete", post(complete_trip))
        .route("/api/v1/trips/{id}/stops/{seq}/arrive", post(stop_arrive))
        .route("/api/v1/trips/{id}/stops/{seq}/depart", post(stop_depart))
        .route("/api/v1/trips/{id}/stops/{seq}/late", post(stop_late))
        .route("/api/v1/trips/{id}/check-call", post(check_call))
}
