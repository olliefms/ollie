//! Surface-agnostic trip-lifecycle business logic.
//!
//! The admin REST API (`/api/v1`), the dispatcher REST API (`/fleet/api/v1`),
//! and the dispatcher MCP server all drive the same trip state machine: assign,
//! unassign, dispatch, undispatch, cancel, complete, plus the late/check-call
//! event emitters. Each surface owns its auth and request-shape concerns; the
//! cascades (resource status, linked-load status), the events, and the re-fetch
//! all live here so every surface behaves identically.

use crate::events;
use crate::models::{DriverStatus, LoadStatus, TrailerStatus, TripRecord, TripStatus, TruckStatus};
use crate::{error::AppError, AppState};
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

pub async fn assign(
    state: &AppState,
    trip_id: Uuid,
    req: AssignTripRequest,
) -> Result<TripRecord, AppError> {
    let driver = state.db.get_driver_by_id(req.driver_id).await?;
    if driver.status == DriverStatus::Inactive {
        return Err(AppError::Conflict(format!("driver {} is not available for assignment", req.driver_id)));
    }

    let truck = state.db.get_truck_by_id(req.truck_id).await?;
    if matches!(truck.status, TruckStatus::OutOfService | TruckStatus::Inactive) {
        return Err(AppError::Conflict(format!("truck {} is not available for assignment", req.truck_id)));
    }

    // Pre-validate all trailers before any mutation to prevent partial state
    let mut trailers = Vec::new();
    for &trailer_id in &req.trailer_ids {
        let trailer = state.db.get_trailer_by_id(trailer_id).await?;
        if !matches!(
            trailer.status,
            TrailerStatus::Available | TrailerStatus::Assigned
        ) {
            return Err(AppError::Conflict(format!(
                "trailer {} is not available for assignment",
                trailer_id
            )));
        }
        trailers.push(trailer);
    }

    state.db.transition_trip_status(trip_id, TripStatus::Assigned).await?;
    state
        .db
        .update_trip_resources(trip_id, Some(req.driver_id), Some(req.truck_id), req.trailer_ids.clone())
        .await?;

    if driver.status == DriverStatus::Available {
        state.db.update_driver_status(req.driver_id, DriverStatus::Assigned).await?;
    }
    if truck.status == TruckStatus::Available {
        state.db.update_truck_status(req.truck_id, TruckStatus::Assigned).await?;
    }
    for trailer in &trailers {
        if trailer.status == TrailerStatus::Available {
            state.db.update_trailer_status(trailer.id, TrailerStatus::Assigned).await?;
        }
    }

    let trip = state.db.get_trip(trip_id).await?;

    if let Some(load_id) = trip.load_id {
        if let Ok(load) = state.db.get_load_by_id(load_id).await {
            if load.status == LoadStatus::Planned {
                let _ = state.db.transition_load_status(load_id, LoadStatus::Assigned, None, None, None).await;
            }
        }
    }

    events::on_trip_assigned(&state.db, trip_id).await;
    Ok(trip)
}

pub async fn unassign(state: &AppState, trip_id: Uuid) -> Result<TripRecord, AppError> {
    let existing = state.db.get_trip(trip_id).await?;
    state.db.transition_trip_status(trip_id, TripStatus::Planned).await?;
    state.db.update_trip_resources(trip_id, None, None, vec![]).await?;

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

    let trip = state.db.get_trip(trip_id).await?;
    events::on_trip_unassigned(&state.db, trip_id).await;
    Ok(trip)
}

pub async fn dispatch(state: &AppState, trip_id: Uuid) -> Result<TripRecord, AppError> {
    let existing = state.db.get_trip(trip_id).await?;
    if existing.status != TripStatus::Assigned {
        return Err(AppError::Conflict("trip must be in assigned status to dispatch".into()));
    }

    let driver_for_dispatch = if let Some(driver_id) = existing.driver_id {
        let driver = state.db.get_driver_by_id(driver_id).await?;
        if driver.status == DriverStatus::Dispatched {
            return Err(AppError::Conflict(
                "driver is already dispatched on another trip".into()
            ));
        }
        Some(driver)
    } else {
        None
    };
    if let Some(truck_id) = existing.truck_id {
        let truck = state.db.get_truck_by_id(truck_id).await?;
        if truck.status == TruckStatus::Dispatched {
            return Err(AppError::Conflict(
                "truck is already dispatched on another trip".into()
            ));
        }
    }

    // Reconcile trip trailers to the driver's currently-attached trailers.
    // Issue #268: at dispatch time, the trip should reflect reality — the trailer
    // physically attached to the driver — not the trailer the trip was created with.
    let mut existing = existing;
    if let Some(driver) = &driver_for_dispatch {
        if !driver.current_trailer_ids.is_empty()
            && driver.current_trailer_ids != existing.trailer_ids
        {
            let dropped: Vec<Uuid> = existing.trailer_ids.iter()
                .filter(|tid| !driver.current_trailer_ids.contains(tid))
                .copied()
                .collect();
            state.db.update_trip_resources(
                existing.id,
                existing.driver_id,
                existing.truck_id,
                driver.current_trailer_ids.clone(),
            ).await?;
            existing.trailer_ids = driver.current_trailer_ids.clone();
            // Trailers that were assigned to this trip but are no longer attached
            // fall back to Available — they're no longer on this load.
            for tid in dropped {
                let _ = state.db.update_trailer_status(tid, TrailerStatus::Available).await;
            }
        }
    }

    let trip = state.db.transition_trip_status(trip_id, TripStatus::Dispatched).await?;

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

    events::on_trip_dispatched(&state.db, trip_id).await;
    Ok(trip)
}

pub async fn undispatch(state: &AppState, trip_id: Uuid) -> Result<TripRecord, AppError> {
    let existing = state.db.get_trip(trip_id).await?;
    if existing.status != TripStatus::Dispatched {
        return Err(AppError::Conflict("trip must be in dispatched status to undispatch".into()));
    }

    let trip = state.db.transition_trip_status(trip_id, TripStatus::Assigned).await?;

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
                t.id != trip_id && (t.status == TripStatus::Dispatched || t.status == TripStatus::InTransit)
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

    events::on_trip_undispatched(&state.db, trip_id).await;
    Ok(trip)
}

pub async fn cancel(state: &AppState, trip_id: Uuid) -> Result<TripRecord, AppError> {
    let existing = state.db.get_trip(trip_id).await?;
    if existing.status == TripStatus::InTransit || existing.status == TripStatus::Delivered {
        return Err(AppError::Conflict("cannot cancel a trip that is in_transit or delivered".into()));
    }

    let trip = state.db.transition_trip_status(trip_id, TripStatus::Cancelled).await?;

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

    events::on_trip_cancelled(&state.db, trip_id).await;
    Ok(trip)
}

/// Completes a delivered trip and releases its resources. Returns `()` because
/// the admin/dispatch surfaces respond 204 No Content.
pub async fn complete(state: &AppState, trip_id: Uuid) -> Result<(), AppError> {
    let existing = state.db.get_trip(trip_id).await?;
    if existing.status != TripStatus::Delivered {
        return Err(AppError::Conflict("trip must be in delivered status to complete".into()));
    }

    state.db.transition_trip_status(trip_id, TripStatus::Completed).await?;

    // Only release a resource to Available if it has NOT already been rebound
    // to another active trip (e.g. via auto-dispatch when this trip delivered).
    let active = list_active_trips(state).await.unwrap_or_default();
    if let Some(driver_id) = existing.driver_id {
        if !resource_on_other_active_trip(&active, trip_id, Some(driver_id), None, None) {
            let _ = state.db.update_driver_status(driver_id, DriverStatus::Available).await;
        }
    }
    if let Some(truck_id) = existing.truck_id {
        if !resource_on_other_active_trip(&active, trip_id, None, Some(truck_id), None) {
            let _ = state.db.update_truck_status(truck_id, TruckStatus::Available).await;
        }
    }
    for &trailer_id in &existing.trailer_ids {
        if !resource_on_other_active_trip(&active, trip_id, None, None, Some(trailer_id)) {
            let _ = state.db.update_trailer_status(trailer_id, TrailerStatus::Available).await;
        }
    }

    events::on_trip_completed(&state.db, trip_id, existing.driver_id, existing.truck_id, &existing.trailer_ids).await;
    Ok(())
}

/// Records a stop-late flag by emitting the `stop.late` event. Verifies the trip
/// exists first.
pub async fn stop_late(
    state: &AppState,
    trip_id: Uuid,
    seq: u32,
    req: StopLateRequest,
) -> Result<(), AppError> {
    state.db.get_trip(trip_id).await?;
    events::on_stop_late(&state.db, trip_id, seq, req.eta, req.notes).await;
    Ok(())
}

/// Records a check call by emitting the `check_call` event. Verifies the trip
/// exists first.
pub async fn check_call(
    state: &AppState,
    trip_id: Uuid,
    req: CheckCallRequest,
) -> Result<(), AppError> {
    state.db.get_trip(trip_id).await?;
    events::on_check_call(&state.db, trip_id, req.location, req.notes, req.eta_next_stop).await;
    Ok(())
}

/// Fetches all trips currently in Dispatched or InTransit status.
async fn list_active_trips(state: &AppState) -> Result<Vec<crate::models::trip::TripListItem>, AppError> {
    let mut out = state.db.list_trips(None, None, Some("dispatched"), None, None).await?;
    out.extend(state.db.list_trips(None, None, Some("in_transit"), None, None).await?);
    Ok(out)
}

/// Returns true if any trip in `active` (other than `exclude_trip_id`)
/// references `resource_id` via the resource-matching closure.
fn resource_on_other_active_trip(
    active: &[crate::models::trip::TripListItem],
    exclude_trip_id: Uuid,
    driver_id: Option<Uuid>,
    truck_id: Option<Uuid>,
    trailer_id: Option<Uuid>,
) -> bool {
    active.iter().any(|t| {
        if t.id == exclude_trip_id { return false; }
        if let Some(d) = driver_id { if t.driver_id == Some(d) { return true; } }
        if let Some(tk) = truck_id { if t.truck_id == Some(tk) { return true; } }
        if let Some(tr) = trailer_id { if t.trailer_ids.contains(&tr) { return true; } }
        false
    })
}

/// After a trip transitions to Delivered, find the driver's next Assigned trip
/// and auto-dispatch it. Best-effort: errors are logged and swallowed so a
/// hiccup here does not break the calling endpoint.
///
/// `dispatch`'s resource-conflict checks are not reused as-is because the
/// driver and truck from the just-delivered trip will still read `Dispatched`.
/// Instead this helper checks whether the candidate trip's truck/trailers are
/// bound to ANOTHER active trip (not the one that just delivered) — if so, it
/// declines to auto-dispatch and leaves the trip Assigned for the dispatcher.
pub(crate) async fn try_auto_dispatch_next_for_driver(
    state: &AppState,
    driver_id: Uuid,
    just_delivered_trip_id: Uuid,
) {
    let Ok(trips) = state.db.list_trips(None, Some(driver_id), Some("assigned"), None, None).await else {
        tracing::warn!(%driver_id, "auto-dispatch: failed to list assigned trips");
        return;
    };
    let mut candidates: Vec<_> = trips.into_iter()
        .filter(|t| t.id != just_delivered_trip_id)
        .collect();
    if candidates.is_empty() { return; }

    candidates.sort_by_key(|t| {
        let origin = t.stops.iter().min_by_key(|s| s.sequence);
        let scheduled = origin.and_then(|s| {
            s.scheduled_arrive.as_deref().and_then(|sa| {
                let parsed = crate::models::load::parse_stop_time(sa, s.timezone.as_deref());
                if parsed.is_none() {
                    tracing::warn!(trip_id = %t.id, sched = %sa, "auto-dispatch: unparseable scheduled_arrive");
                }
                parsed
            })
        });
        (scheduled.unwrap_or(chrono::DateTime::<chrono::Utc>::MAX_UTC), t.created_at)
    });

    let next = &candidates[0];
    let trip_id = next.id;

    // Refuse to bind a truck or trailer that is already active on another trip.
    // The driver is exempt — they were on the just-delivered trip; their status
    // still reads Dispatched but that does not count as a different trip.
    let active = match list_active_trips(state).await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(%trip_id, error = %e, "auto-dispatch: failed to list active trips");
            return;
        }
    };
    if let Some(truck_id) = next.truck_id {
        if resource_on_other_active_trip(&active, just_delivered_trip_id, None, Some(truck_id), None) {
            tracing::warn!(%trip_id, %truck_id, "auto-dispatch: truck busy on another active trip, skipping");
            return;
        }
    }
    for &trailer_id in &next.trailer_ids {
        if resource_on_other_active_trip(&active, just_delivered_trip_id, None, None, Some(trailer_id)) {
            tracing::warn!(%trip_id, %trailer_id, "auto-dispatch: trailer busy on another active trip, skipping");
            return;
        }
    }

    if let Err(e) = state.db.transition_trip_status(trip_id, TripStatus::Dispatched).await {
        tracing::warn!(%trip_id, error = %e, "auto-dispatch: trip state transition failed");
        return;
    }

    let _ = state.db.update_driver_status(driver_id, DriverStatus::Dispatched).await;
    if let Some(truck_id) = next.truck_id {
        let _ = state.db.update_truck_status(truck_id, TruckStatus::Dispatched).await;
    }
    for &trailer_id in &next.trailer_ids {
        let _ = state.db.update_trailer_status(trailer_id, TrailerStatus::Dispatched).await;
    }

    if let Some(load_id) = next.load_id {
        if let Ok(load) = state.db.get_load_by_id(load_id).await {
            if load.status == LoadStatus::Assigned {
                let _ = state.db.transition_load_status(
                    load_id, LoadStatus::Dispatched, None, None, None,
                ).await;
            }
        }
    }

    tracing::info!(prev_trip = %just_delivered_trip_id, next_trip = %trip_id, %driver_id, "auto-dispatched next trip");
    events::on_trip_dispatched(&state.db, trip_id).await;
}
