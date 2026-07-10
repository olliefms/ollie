//! Cross-surface stop-time operations.
//!
//! Both the fleet portal (POST /fleet/api/v1/trips/{id}/stops/{seq}/arrive|depart)
//! and the driver portal (PATCH /driver/api/v1/trips/{id}/stops/{seq}) record actual
//! arrival and departure times. The cascade to the linked load stop, the trip/load
//! status transitions, the auto-dispatch of the next assigned trip, and the event
//! hooks must behave identically regardless of which surface initiated the call.
//!
//! Driver/fleet_user handlers own auth and request-shape validation; everything
//! downstream of `db.update_trip_stop` lives here.

use crate::events;
use crate::models::{LoadStatus, TripRecord, TripStatus, TripStopType};
use crate::{error::AppError, AppState};
use uuid::Uuid;

/// Validate `actual_arrive` against the stop's timezone if one is set.
/// Handlers should call this before persisting so the 422 surfaces cleanly.
pub fn validate_arrive(actual_arrive: &str, tz: Option<&str>) -> Result<(), AppError> {
    if let Some(tz_str) = tz {
        crate::models::load::validate_stop_time_str(actual_arrive, tz_str, "actual_arrive")?;
    }
    Ok(())
}

/// Validate `actual_depart` against the stop's timezone if one is set.
pub fn validate_depart(actual_depart: &str, tz: Option<&str>) -> Result<(), AppError> {
    if let Some(tz_str) = tz {
        crate::models::load::validate_stop_time_str(actual_depart, tz_str, "actual_depart")?;
    }
    Ok(())
}

/// Record an actual-arrival time for a trip stop, cascading to the linked load
/// stop and firing `stop.arrived`. Returns the updated trip as written to the db
/// (cascade-side updates are persisted but not re-read).
pub async fn record_stop_arrive(
    state: &AppState,
    trip_id: Uuid,
    seq: u32,
    actual_arrive: String,
) -> Result<TripRecord, AppError> {
    let trip = state
        .db
        .update_trip_stop(trip_id, seq, Some(actual_arrive.clone()), None)
        .await?;

    cascade_load_stop_arrive(state, &trip, seq, &actual_arrive).await;

    events::on_stop_arrived(&state.db, trip_id, seq).await;
    Ok(trip)
}

/// Record an actual-departure time for a trip stop. In addition to the load-stop
/// cascade and `stop.departed` event, this advances trip + load status:
///
/// * First transit-starting depart on a `Dispatched` trip → trip becomes
///   `InTransit`, and the load follows if it is still `Dispatched`. For a loaded
///   trip that stop is the first `Pickup`; for a non-freight trip (a
///   repositioning/empty/maintenance move with no pickup) it is the first stop.
/// * Final stop depart on an `InTransit` trip → trip becomes `Delivered`, the
///   load follows when all of its trips are `Delivered`, and the driver's next
///   `Assigned` trip is auto-dispatched.
///
/// A single-stop non-freight trip (e.g. one `terminal` stop) advances through
/// both cascades on the one depart: the first sets `InTransit`, the second
/// re-reads that status and sets `Delivered`, giving empty moves a real
/// completion path.
///
/// Returns a re-fetched trip reflecting any status transitions.
pub async fn record_stop_depart(
    state: &AppState,
    trip_id: Uuid,
    seq: u32,
    actual_depart: String,
) -> Result<TripRecord, AppError> {
    let trip = state
        .db
        .update_trip_stop(trip_id, seq, None, Some(actual_depart.clone()))
        .await?;

    cascade_load_stop_depart(state, &trip, seq, &actual_depart).await;
    cascade_start_in_transit(state, &trip, seq).await;
    cascade_final_stop_delivered(state, trip_id, seq).await;

    events::on_stop_departed(&state.db, trip_id, seq).await;

    state.db.get_trip(trip_id).await
}

/// Clear recorded actual times on a trip stop, cascading the clear to the linked
/// load stop. Clearing arrival also clears departure. Status transitions are not
/// rewound — clearing is a data correction. Returns a re-fetched trip.
pub async fn clear_stop_times(
    state: &AppState,
    trip_id: Uuid,
    seq: u32,
    clear_arrive: bool,
    clear_depart: bool,
) -> Result<TripRecord, AppError> {
    let trip = state
        .db
        .clear_trip_stop_times(trip_id, seq, clear_arrive, clear_depart)
        .await?;

    cascade_load_stop_clear(state, &trip, seq, clear_arrive, clear_depart).await;

    state.db.get_trip(trip_id).await
}

// ---------------------------------------------------------------------------
// internal cascade helpers
// ---------------------------------------------------------------------------

async fn cascade_load_stop_clear(
    state: &AppState, trip: &TripRecord, seq: u32,
    clear_arrive: bool, clear_depart: bool,
) {
    let Some(load_id) = trip.load_id else { return };
    let Some(stop) = trip.stops.iter().find(|s| s.sequence == seq) else { return };
    let Some(load_stop_idx) = stop.load_stop_index else { return };
    let Ok(load) = state.db.get_load_by_id(load_id).await else { return };
    let mut updated_stops = load.stops.clone();
    if (load_stop_idx as usize) >= updated_stops.len() {
        return;
    }
    if clear_arrive {
        updated_stops[load_stop_idx as usize].actual_arrive = None;
        updated_stops[load_stop_idx as usize].actual_depart = None;
    } else if clear_depart {
        updated_stops[load_stop_idx as usize].actual_depart = None;
    }
    let _ = state
        .db
        .update_load_metadata(
            load_id, None, None, Some(updated_stops),
            None, None, None, None, None, None, None, None,
        )
        .await;
}

async fn cascade_load_stop_arrive(state: &AppState, trip: &TripRecord, seq: u32, value: &str) {
    let Some(load_id) = trip.load_id else { return };
    let Some(stop) = trip.stops.iter().find(|s| s.sequence == seq) else { return };
    let Some(load_stop_idx) = stop.load_stop_index else { return };
    let Ok(load) = state.db.get_load_by_id(load_id).await else { return };
    let mut updated_stops = load.stops.clone();
    if (load_stop_idx as usize) >= updated_stops.len() {
        return;
    }
    updated_stops[load_stop_idx as usize].actual_arrive = Some(value.to_string());
    let _ = state
        .db
        .update_load_metadata(
            load_id, None, None, Some(updated_stops),
            None, None, None, None, None, None, None, None,
        )
        .await;
}

async fn cascade_load_stop_depart(state: &AppState, trip: &TripRecord, seq: u32, value: &str) {
    let Some(load_id) = trip.load_id else { return };
    let Some(stop) = trip.stops.iter().find(|s| s.sequence == seq) else { return };
    let Some(load_stop_idx) = stop.load_stop_index else { return };
    let Ok(load) = state.db.get_load_by_id(load_id).await else { return };
    let mut updated_stops = load.stops.clone();
    if (load_stop_idx as usize) >= updated_stops.len() {
        return;
    }
    updated_stops[load_stop_idx as usize].actual_depart = Some(value.to_string());
    let _ = state
        .db
        .update_load_metadata(
            load_id, None, None, Some(updated_stops),
            None, None, None, None, None, None, None, None,
        )
        .await;
}

async fn cascade_start_in_transit(state: &AppState, trip: &TripRecord, seq: u32) {
    let Some(stop) = trip.stops.iter().find(|s| s.sequence == seq) else { return };
    if trip.status != TripStatus::Dispatched {
        return;
    }
    // A loaded trip starts transit only when its first `Pickup` departs. A
    // non-freight trip (repositioning/empty/maintenance move with no pickup)
    // has no pickup to gate on, so its first stop's departure starts transit —
    // otherwise such a trip could never leave `Dispatched` and never complete.
    let has_pickup = trip.stops.iter().any(|s| s.stop_type == TripStopType::Pickup);
    let starts_transit = if has_pickup {
        stop.stop_type == TripStopType::Pickup
    } else {
        trip.stops.iter().map(|s| s.sequence).min() == Some(seq)
    };
    if !starts_transit {
        return;
    }
    if state
        .db
        .transition_trip_status(trip.id, TripStatus::InTransit)
        .await
        .is_err()
    {
        return;
    }
    if let Some(load_id) = trip.load_id {
        if let Ok(load) = state.db.get_load_by_id(load_id).await {
            if load.status == LoadStatus::Dispatched {
                let _ = state
                    .db
                    .transition_load_status(load_id, LoadStatus::InTransit, None, None, None)
                    .await;
            }
        }
    }
    events::on_trip_in_transit(&state.db, trip.id).await;
}

async fn cascade_final_stop_delivered(state: &AppState, trip_id: Uuid, seq: u32) {
    let Ok(current) = state.db.get_trip(trip_id).await else { return };
    let max_seq = current.stops.iter().map(|s| s.sequence).max();
    if Some(seq) != max_seq || current.status != TripStatus::InTransit {
        return;
    }
    let Ok(delivered) = state
        .db
        .transition_trip_status(trip_id, TripStatus::Delivered)
        .await
    else {
        return;
    };
    events::on_trip_delivered(&state.db, trip_id).await;

    if let Some(load_id) = delivered.load_id {
        if let Ok(trips) = state.db.list_trips_for_load(load_id).await {
            if trips.iter().all(|t| t.status == TripStatus::Delivered) {
                if let Ok(load) = state.db.get_load_by_id(load_id).await {
                    if load.status == LoadStatus::InTransit {
                        let _ = state
                            .db
                            .transition_load_status(load_id, LoadStatus::Delivered, None, None, None)
                            .await;
                    }
                }
            }
        }
    }

    if let Some(driver_id) = delivered.driver_id {
        crate::services::trip_lifecycle::try_auto_dispatch_next_for_driver(state, driver_id, trip_id).await;
    }
}
