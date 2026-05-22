// src/api/mileage_summary.rs
//
// Builds the unified MileageSummary block surfaced on driver + dispatcher trip detail
// responses. Resolves deadhead-origin facility metadata and zips per-leg miles with
// stop names.

use crate::models::trip::{DeadheadOrigin, LegMiles, MileageSummary, TripRecord};
use crate::AppState;
use std::collections::HashMap;
use uuid::Uuid;

/// Build a MileageSummary from a TripRecord. Reads the previous trip + facility metadata
/// for the deadhead origin label. Per-leg breakdown comes from the persisted
/// `segment_miles` vec, zipped with stop names — one leg per consecutive waypoint pair.
pub async fn build_mileage_summary(
    state: &AppState,
    trip: &TripRecord,
) -> MileageSummary {
    let origin = resolve_deadhead_origin(state, trip).await;
    let has_deadhead_leg = origin.is_some();

    // Build the ordered waypoint name list matching how segment_miles was computed:
    // [origin_name?, stop_0_name, stop_1_name, ...]
    let mut waypoint_names: Vec<Option<String>> = Vec::new();
    if let Some(o) = &origin {
        waypoint_names.push(o.facility_name.clone());
    }
    for s in &trip.stops {
        waypoint_names.push(s.name.clone());
    }

    // Zip segment_miles with consecutive waypoint pairs.
    let mut legs: Vec<LegMiles> = Vec::new();
    for (i, miles) in trip.segment_miles.iter().enumerate() {
        let from = waypoint_names.get(i).cloned().flatten();
        let to = waypoint_names.get(i + 1).cloned().flatten();
        let kind = if has_deadhead_leg && i == 0 { "deadhead" } else { "loaded" };
        legs.push(LegMiles {
            index: i as u32,
            kind: kind.into(),
            from,
            to,
            miles: Some(*miles),
        });
    }

    MileageSummary {
        origin,
        legs,
        deadhead_miles: trip.deadhead_miles,
        loaded_miles: trip.loaded_miles,
        total_miles: trip.total_miles
            .or(match (trip.deadhead_miles, trip.loaded_miles) {
                (Some(d), Some(l)) => Some(d + l),
                (Some(x), None) | (None, Some(x)) => Some(x),
                (None, None) => None,
            }),
    }
}

/// Build the load-level MileageSummary: loaded miles only, no deadhead/origin.
/// Picks the most-recent non-cancelled trip linked to this load and strips its
/// deadhead leg + origin. Returns None when no eligible trip exists.
pub async fn build_load_mileage_summary(
    state: &AppState,
    load_id: Uuid,
) -> Option<MileageSummary> {
    let mut trips = state.db.list_trips_for_load(load_id).await.ok()?;
    trips.retain(|t| t.status != crate::models::TripStatus::Cancelled);
    if trips.is_empty() {
        return None;
    }
    trips.sort_by_key(|t| std::cmp::Reverse(t.created_at));
    let trip = trips.into_iter().next()?;
    let summary = build_mileage_summary(state, &trip).await;
    let legs: Vec<LegMiles> = summary.legs.into_iter()
        .filter(|l| l.kind != "deadhead")
        .collect();
    let total_miles = summary.loaded_miles;
    Some(MileageSummary {
        origin: None,
        legs,
        deadhead_miles: None,
        loaded_miles: summary.loaded_miles,
        total_miles,
    })
}

async fn resolve_deadhead_origin(
    state: &AppState,
    trip: &TripRecord,
) -> Option<DeadheadOrigin> {
    let prev_id = trip.previous_trip_id?;
    let prev = state.db.get_trip(prev_id).await.ok()?;
    let last_stop = prev.stops.last()?;
    let fac_id = last_stop.facility_id?;
    let facilities: HashMap<Uuid, crate::models::FacilityRecord> =
        state.db.batch_get_facilities(&[fac_id]).await.ok()?;
    let fac = facilities.get(&fac_id)?;
    Some(DeadheadOrigin {
        trip_id: prev_id,
        facility_name: Some(fac.name.clone()),
        address: fac.normalized_address.clone().or_else(|| Some(fac.address.clone())),
    })
}
