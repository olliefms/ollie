// src/api/driver_portal/data.rs
use axum::{
    Extension,
    Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    AppState,
    api::driver_portal::jwt::DriverClaims,
    error::AppError,
    models::{TripListItem, TripStatus},
};

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct DriverMeResponse {
    pub id: Uuid,
    pub name: String,
    pub phone: Option<String>,
    pub status: String,
}

#[derive(Serialize)]
pub struct DriverTripListItem {
    pub id: Uuid,
    pub trip_number: String,
    pub status: String,
    pub origin: String,
    pub destination: String,
    pub stop_count: usize,
    pub stops_completed: usize,
    pub scheduled_start: Option<String>,
    pub truck_unit: Option<String>,
    pub trailer_units: Vec<String>,
    pub next_stop_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivered_tz: Option<String>,
}

#[derive(Serialize)]
pub struct DriverTripListResponse {
    pub items: Vec<DriverTripListItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub week: Option<PastWeekInfo>,
}

#[derive(Serialize)]
pub struct PastWeekInfo {
    pub start: String,
    pub end: String,
    pub has_prev: bool,
    pub has_next: bool,
    pub earliest_week_start: Option<String>,
    pub latest_week_start: Option<String>,
}

#[derive(Serialize)]
pub struct DriverTripLoadSummary {
    pub load_number: Option<String>,
    pub customer_ref: Option<String>,
    pub commodity: Option<String>,
    pub weight_lbs: Option<f64>,
    pub notes: Option<String>,
}

#[derive(Serialize)]
pub struct DriverTripStopSummary {
    pub sequence: u32,
    pub stop_type: String,
    pub name: String,
    pub address: Option<String>,
    pub scheduled_arrive: Option<String>,
    pub scheduled_arrive_end: Option<String>,
    pub actual_arrive: Option<String>,
    pub actual_depart: Option<String>,
    pub expected_dwell_minutes: Option<u32>,
    pub notes: Option<String>,
    pub timezone: Option<String>,
}

#[derive(Serialize)]
pub struct DriverTripDetailResponse {
    pub id: Uuid,
    pub trip_number: String,
    pub status: String,
    pub truck_unit: Option<String>,
    pub trailer_units: Vec<String>,
    pub load: Option<DriverTripLoadSummary>,
    pub stops: Vec<DriverTripStopSummary>,
}

#[derive(Serialize)]
pub struct DriverStopAddress {
    pub street: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub zip: Option<String>,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct DriverFacilityContact {
    pub name: String,
    pub title: Option<String>,
    pub phone: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdateStopTimesRequest {
    #[serde(default)]
    pub actual_arrive: Option<String>,
    #[serde(default)]
    pub actual_depart: Option<String>,
}

#[derive(Serialize)]
pub struct DriverStopDetailResponse {
    pub sequence: u32,
    pub stop_type: String,
    pub facility_name: Option<String>,
    pub address: Option<DriverStopAddress>,
    pub scheduled_arrive: Option<String>,
    pub scheduled_arrive_end: Option<String>,
    pub actual_arrive: Option<String>,
    pub actual_depart: Option<String>,
    pub expected_dwell_minutes: Option<u32>,
    pub commodity: Option<String>,
    pub weight_lbs: Option<f64>,
    pub notes: Option<String>,
    pub contacts: Vec<DriverFacilityContact>,
    pub timezone: Option<String>,
}

// ---------------------------------------------------------------------------
// Tab classification
// ---------------------------------------------------------------------------

pub enum TripTab {
    Past,
    Current,
    Upcoming,
}

pub fn classify_trip(trip: &TripListItem) -> TripTab {
    use crate::models::load::parse_stop_time;
    match &trip.status {
        TripStatus::Delivered | TripStatus::Completed | TripStatus::Cancelled => TripTab::Past,
        TripStatus::Dispatched | TripStatus::InTransit => TripTab::Current,
        TripStatus::Assigned => {
            let first_stop = trip.stops.first();
            let parsed = first_stop.and_then(|s| {
                s.scheduled_arrive
                    .as_deref()
                    .and_then(|sa| parse_stop_time(sa, s.timezone.as_deref()))
            });
            match parsed {
                Some(dt) if dt <= chrono::Utc::now() => TripTab::Current,
                // Safe default per AGENTS.md: pre-departure trips go to Upcoming.
                _ => TripTab::Upcoming,
            }
        }
        TripStatus::Planned => TripTab::Upcoming,
    }
}

fn tab_matches(tab: &TripTab, trip: &TripListItem) -> bool {
    matches!(
        (tab, classify_trip(trip)),
        (TripTab::Past, TripTab::Past)
            | (TripTab::Current, TripTab::Current)
            | (TripTab::Upcoming, TripTab::Upcoming)
    )
}

// ---------------------------------------------------------------------------
// Query params
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct TripsQuery {
    pub tab: Option<String>,
    pub week_start: Option<String>,
}

fn trip_anchor_utc(trip: &TripListItem) -> Option<chrono::DateTime<chrono::Utc>> {
    use crate::models::load::parse_stop_time;
    if let Some(s) = trip.stops.last() {
        if let Some(d) = s.actual_depart.as_deref() {
            if let Some(dt) = parse_stop_time(d, s.timezone.as_deref()) {
                return Some(dt);
            }
        }
    }
    if let Some(s) = trip.stops.first() {
        if let Some(sa) = s.scheduled_arrive.as_deref() {
            if let Some(dt) = parse_stop_time(sa, s.timezone.as_deref()) {
                return Some(dt);
            }
        }
    }
    if matches!(trip.status, crate::models::TripStatus::Cancelled) {
        return Some(trip.updated_at);
    }
    None
}

fn parse_week_start(
    s: &str,
    tz: chrono_tz::Tz,
) -> Result<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>, chrono::NaiveDate), AppError> {
    use chrono::TimeZone as _;
    let d = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| AppError::BadRequest(format!("week_start must be YYYY-MM-DD, got '{s}'")))?;
    let dt_naive = d.and_hms_opt(0, 0, 0).unwrap();
    let lo = tz
        .from_local_datetime(&dt_naive)
        .single()
        .ok_or_else(|| AppError::BadRequest(format!("week_start '{s}' is ambiguous/nonexistent in terminal tz")))?
        .with_timezone(&chrono::Utc);
    // Compute the upper bound from the next Sunday's local midnight rather than
    // lo + 168h so DST transition weeks (one Saturday/spring, one Saturday/fall)
    // bucket correctly. Fall back to the naive 7-day delta only if the next
    // Sunday is somehow ambiguous in this tz.
    let next_naive = (d + chrono::Duration::days(7)).and_hms_opt(0, 0, 0).unwrap();
    let hi = tz
        .from_local_datetime(&next_naive)
        .single()
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or(lo + chrono::Duration::days(7));
    Ok((lo, hi, d))
}

fn sunday_of(d: chrono::NaiveDate) -> chrono::NaiveDate {
    use chrono::Datelike as _;
    let dow = d.weekday().num_days_from_sunday();
    d - chrono::Duration::days(dow as i64)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

pub async fn me(
    State(state): State<AppState>,
    Extension(claims): Extension<DriverClaims>,
) -> Result<impl IntoResponse, AppError> {
    let driver_id = claims.driver_id.parse::<Uuid>().map_err(|_| AppError::Unauthorized)?;
    let driver = state.db.get_driver_by_id(driver_id).await?;
    Ok(Json(DriverMeResponse {
        id: driver.id,
        name: driver.name,
        phone: driver.phone,
        status: driver.status.as_str().to_string(),
    }))
}

pub async fn list_trips(
    State(state): State<AppState>,
    Extension(claims): Extension<DriverClaims>,
    Query(params): Query<TripsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let driver_id = claims.driver_id.parse::<Uuid>().map_err(|_| AppError::Unauthorized)?;

    let tab = match params.tab.as_deref().unwrap_or("current") {
        "past" => TripTab::Past,
        "upcoming" => TripTab::Upcoming,
        _ => TripTab::Current,
    };

    let terminal_tz: chrono_tz::Tz = state
        .config
        .terminal_timezone
        .parse()
        .map_err(|_| AppError::Internal("invalid terminal timezone in config".into()))?;

    let all_trips = state.db.list_trips(None, Some(driver_id), None).await?;

    let (filtered, week_info): (Vec<TripListItem>, Option<PastWeekInfo>) = match tab {
        TripTab::Past => {
            let past_only: Vec<&TripListItem> = all_trips
                .iter()
                .filter(|t| matches!(classify_trip(t), TripTab::Past))
                .collect();
            let mut anchors: Vec<(chrono::DateTime<chrono::Utc>, &TripListItem)> = past_only
                .iter()
                .filter_map(|t| trip_anchor_utc(t).map(|a| (a, *t)))
                .collect();
            anchors.sort_by_key(|(a, _)| *a);

            let now_local = chrono::Utc::now().with_timezone(&terminal_tz).date_naive();
            let default_start = sunday_of(now_local);
            let start_str = params
                .week_start
                .clone()
                .unwrap_or_else(|| default_start.format("%Y-%m-%d").to_string());
            let (lo, hi, start_date) = parse_week_start(&start_str, terminal_tz)?;
            let end_date = start_date + chrono::Duration::days(6);

            let in_week: Vec<TripListItem> = anchors
                .iter()
                .filter(|(a, _)| *a >= lo && *a < hi)
                .map(|(_, t)| (*t).clone())
                .collect();

            let has_prev = anchors.iter().any(|(a, _)| *a < lo);
            let has_next = anchors.iter().any(|(a, _)| *a >= hi);

            let bound_week = |a: &chrono::DateTime<chrono::Utc>| -> chrono::NaiveDate {
                sunday_of(a.with_timezone(&terminal_tz).date_naive())
            };
            let earliest = anchors
                .first()
                .map(|(a, _)| bound_week(a).format("%Y-%m-%d").to_string());
            let latest = anchors
                .last()
                .map(|(a, _)| bound_week(a).format("%Y-%m-%d").to_string());

            let info = PastWeekInfo {
                start: start_date.format("%Y-%m-%d").to_string(),
                end: end_date.format("%Y-%m-%d").to_string(),
                has_prev,
                has_next,
                earliest_week_start: earliest,
                latest_week_start: latest,
            };
            (in_week, Some(info))
        }
        _ => {
            let filtered: Vec<TripListItem> = all_trips
                .iter()
                .filter(|t| tab_matches(&tab, t))
                .cloned()
                .collect();
            (filtered, None)
        }
    };

    // Pre-fetch all facilities needed across all trips in a single batch query
    // (origin stop, destination stop, and next-stop per trip) instead of O(3N) individual calls.
    let all_fac_ids: Vec<Uuid> = {
        let mut seen = std::collections::HashSet::new();
        for trip in &filtered {
            for stop in trip.stops.first().into_iter().chain(trip.stops.last()) {
                if let Some(fid) = stop.facility_id {
                    seen.insert(fid);
                }
            }
            if let Some(next) = trip.stops.iter().find(|s| s.actual_arrive.is_none()) {
                if let Some(fid) = next.facility_id {
                    seen.insert(fid);
                }
            }
        }
        seen.into_iter().collect()
    };
    let fac_map = state.db.batch_get_facilities(&all_fac_ids).await.unwrap_or_default();

    // Resolve facility names synchronously from the pre-fetched map (no DB calls needed).
    let facility_names: Vec<(String, String, Option<String>)> = filtered
        .iter()
        .map(|trip| {
            let origin = resolve_stop_name_sync(&fac_map, trip.stops.first());
            let destination = resolve_stop_name_sync(&fac_map, trip.stops.last());
            let next_stop_name = trip.stops.iter().find(|s| s.actual_arrive.is_none()).and_then(|s| {
                if let Some(fid) = s.facility_id {
                    fac_map.get(&fid).map(|f| f.name.clone())
                } else {
                    s.name.clone()
                }
            });
            (origin, destination, next_stop_name)
        })
        .collect();

    // Use join_all only for the remaining async truck/trailer lookups.
    let async_parts = join_all(filtered.iter().map(|trip| {
        let state = state.clone();
        async move {
            let truck_unit = if let Some(tid) = trip.truck_id {
                state.db.get_truck_by_id(tid).await.ok().map(|t| t.unit_number)
            } else {
                None
            };

            let trailer_units = join_all(
                trip.trailer_ids.iter().map(|tid| {
                    let state = state.clone();
                    let tid = *tid;
                    async move { state.db.get_trailer_by_id(tid).await.ok().map(|t| t.unit_number) }
                })
            )
            .await
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();

            (truck_unit, trailer_units)
        }
    }))
    .await;

    let mut items: Vec<DriverTripListItem> = filtered
        .iter()
        .zip(facility_names)
        .zip(async_parts)
        .map(|((trip, (origin, destination, next_stop_name)), (truck_unit, trailer_units))| {
            let stops_completed = trip.stops.iter().filter(|s| s.actual_depart.is_some()).count();
            let scheduled_start = trip.stops.first().and_then(|s| s.scheduled_arrive.clone());
            let last_stop = trip.stops.last();
            let delivered_at = last_stop.and_then(|s| s.actual_depart.clone());
            let delivered_tz = last_stop.and_then(|s| s.timezone.clone());
            DriverTripListItem {
                id: trip.id,
                trip_number: trip.trip_number.clone(),
                status: trip.status.as_str().to_string(),
                origin,
                destination,
                stop_count: trip.stops.len(),
                stops_completed,
                scheduled_start,
                truck_unit,
                trailer_units,
                next_stop_name,
                delivered_at,
                delivered_tz,
            }
        })
        .collect();

    // Sort past trips by delivered_at descending (newest delivered first).
    if matches!(tab, TripTab::Past) {
        items.sort_by(|a, b| {
            b.delivered_at.as_deref().unwrap_or("").cmp(a.delivered_at.as_deref().unwrap_or(""))
        });
    }

    Ok(Json(DriverTripListResponse { items, week: week_info }))
}

pub async fn trip_detail(
    State(state): State<AppState>,
    Extension(claims): Extension<DriverClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let driver_id = claims.driver_id.parse::<Uuid>().map_err(|_| AppError::Unauthorized)?;

    let trip = state.db.get_trip(id).await?;
    if trip.driver_id != Some(driver_id) {
        return Err(AppError::Unauthorized);
    }

    let facility_ids: Vec<Uuid> = trip
        .stops
        .iter()
        .filter_map(|s| s.facility_id)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    let load_fut = async {
        if let Some(lid) = trip.load_id {
            state.db.get_load_by_id(lid).await.map(Some)
        } else {
            Ok(None)
        }
    };
    let facilities_fut = join_all(facility_ids.iter().map(|fid| state.db.get_facility_by_id(*fid)));

    let (load_opt, facility_results) = tokio::try_join!(load_fut, async { Ok(facilities_fut.await) })?;
    let facilities: HashMap<Uuid, crate::models::FacilityRecord> = facility_ids
        .iter()
        .zip(facility_results)
        .filter_map(|(id, r)| r.ok().map(|f| (*id, f)))
        .collect();

    let (truck_opt, trailer_results) = tokio::try_join!(
        async {
            if let Some(tid) = trip.truck_id {
                state.db.get_truck_by_id(tid).await.map(Some)
            } else {
                Ok(None)
            }
        },
        async { Ok(join_all(trip.trailer_ids.iter().map(|tid| state.db.get_trailer_by_id(*tid))).await) },
    )?;

    let truck_unit = truck_opt.map(|t| t.unit_number);
    let trailer_units = trailer_results.into_iter().filter_map(|r| r.ok().map(|t| t.unit_number)).collect();

    let load = load_opt.map(|l| DriverTripLoadSummary {
        load_number: Some(l.load_number.clone()),
        customer_ref: l.customer_ref,
        commodity: l.commodity,
        weight_lbs: l.weight_lbs,
        notes: l.notes,
    });

    let stops = trip
        .stops
        .iter()
        .map(|s| {
            let (name, address) = if let Some(fid) = s.facility_id {
                if let Some(f) = facilities.get(&fid) {
                    (f.name.clone(), Some(f.address.clone()))
                } else {
                    (s.name.clone().unwrap_or_else(|| format!("Stop {}", s.sequence)), None)
                }
            } else {
                (s.name.clone().unwrap_or_else(|| format!("Stop {}", s.sequence)), None)
            };
            DriverTripStopSummary {
                sequence: s.sequence,
                stop_type: s.stop_type.as_str().to_string(),
                name,
                address,
                scheduled_arrive: s.scheduled_arrive.clone(),
                scheduled_arrive_end: s.scheduled_arrive_end.clone(),
                actual_arrive: s.actual_arrive.clone(),
                actual_depart: s.actual_depart.clone(),
                expected_dwell_minutes: s.expected_dwell_minutes,
                notes: s.notes.clone(),
                timezone: s.timezone.clone(),
            }
        })
        .collect();

    Ok(Json(DriverTripDetailResponse {
        id: trip.id,
        trip_number: trip.trip_number,
        status: trip.status.as_str().to_string(),
        truck_unit,
        trailer_units,
        load,
        stops,
    }))
}

pub async fn stop_detail(
    State(state): State<AppState>,
    Extension(claims): Extension<DriverClaims>,
    Path((id, seq)): Path<(Uuid, u32)>,
) -> Result<impl IntoResponse, AppError> {
    let driver_id = claims.driver_id.parse::<Uuid>().map_err(|_| AppError::Unauthorized)?;

    let trip = state.db.get_trip(id).await?;
    if trip.driver_id != Some(driver_id) {
        return Err(AppError::Unauthorized);
    }

    let stop = trip.stops.iter().find(|s| s.sequence == seq).ok_or(AppError::NotFound)?;

    let (facility_opt, load_opt) = tokio::try_join!(
        async {
            if let Some(fid) = stop.facility_id {
                state.db.get_facility_by_id(fid).await.map(Some)
            } else {
                Ok(None)
            }
        },
        async {
            if let Some(lid) = trip.load_id {
                state.db.get_load_by_id(lid).await.map(Some)
            } else {
                Ok(None)
            }
        },
    )?;

    let facility_name = facility_opt.as_ref().map(|f| f.name.clone());
    let contacts: Vec<DriverFacilityContact> = facility_opt
        .as_ref()
        .map(|f| {
            f.contacts
                .iter()
                .filter_map(|c| {
                    c.phone.as_ref().map(|phone| DriverFacilityContact {
                        name: c.name.clone(),
                        title: c.title.clone(),
                        phone: phone.clone(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let address = facility_opt.map(|f| DriverStopAddress {
        street: Some(f.address.clone()),
        city: None,
        state: None,
        zip: None,
    });

    Ok(Json(DriverStopDetailResponse {
        sequence: stop.sequence,
        stop_type: stop.stop_type.as_str().to_string(),
        facility_name,
        address,
        scheduled_arrive: stop.scheduled_arrive.clone(),
        scheduled_arrive_end: stop.scheduled_arrive_end.clone(),
        actual_arrive: stop.actual_arrive.clone(),
        actual_depart: stop.actual_depart.clone(),
        expected_dwell_minutes: stop.expected_dwell_minutes,
        commodity: load_opt.as_ref().and_then(|l| l.commodity.clone()),
        weight_lbs: load_opt.as_ref().and_then(|l| l.weight_lbs),
        notes: stop.notes.clone(),
        contacts,
        timezone: stop.timezone.clone(),
    }))
}

#[utoipa::path(
    patch,
    path = "/driver/api/v1/trips/{id}/stops/{seq}",
    request_body = UpdateStopTimesRequest,
    responses(
        (status = 200, description = "Stop updated"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Trip or stop not found"),
        (status = 422, description = "Validation failed"),
    ),
    security(("BearerAuth" = [])),
    tag = "driver"
)]
pub async fn update_stop_times(
    State(state): State<AppState>,
    Extension(claims): Extension<DriverClaims>,
    Path((id, seq)): Path<(Uuid, u32)>,
    Json(req): Json<UpdateStopTimesRequest>,
) -> Result<impl IntoResponse, AppError> {
    use crate::models::load::{parse_stop_time, validate_stop_time_str};
    let driver_id = claims.driver_id.parse::<Uuid>().map_err(|_| AppError::Unauthorized)?;
    let trip = state.db.get_trip(id).await?;
    if trip.driver_id != Some(driver_id) {
        return Err(AppError::NotFound);
    }

    // Stop sequence is 1-based; array index is 0-based (AGENTS.md line 327).
    let stop_idx = trip.stops.iter().position(|s| s.sequence == seq).ok_or(AppError::NotFound)?;
    let tz = trip.stops[stop_idx].timezone.clone();

    if let Some(tz_str) = tz.as_deref() {
        if let Some(a) = req.actual_arrive.as_deref() {
            validate_stop_time_str(a, tz_str, "actual_arrive")?;
        }
        if let Some(d) = req.actual_depart.as_deref() {
            validate_stop_time_str(d, tz_str, "actual_depart")?;
        }
    }

    let next_arrive = req.actual_arrive.clone().or_else(|| trip.stops[stop_idx].actual_arrive.clone());
    let next_depart = req.actual_depart.clone().or_else(|| trip.stops[stop_idx].actual_depart.clone());

    if next_depart.is_some() && next_arrive.is_none() {
        return Err(AppError::UnprocessableEntity("actual_depart requires actual_arrive".into()));
    }

    if let (Some(a), Some(d)) = (next_arrive.as_deref(), next_depart.as_deref()) {
        let a_utc = parse_stop_time(a, tz.as_deref())
            .ok_or_else(|| AppError::UnprocessableEntity("unparseable actual_arrive".into()))?;
        let d_utc = parse_stop_time(d, tz.as_deref())
            .ok_or_else(|| AppError::UnprocessableEntity("unparseable actual_depart".into()))?;
        if d_utc < a_utc {
            return Err(AppError::UnprocessableEntity("actual_depart must be >= actual_arrive".into()));
        }
    }

    let skew_check = |s: &str| -> Result<(), AppError> {
        let dt = parse_stop_time(s, tz.as_deref())
            .ok_or_else(|| AppError::UnprocessableEntity("unparseable timestamp".into()))?;
        if dt > chrono::Utc::now() + chrono::Duration::hours(24) {
            return Err(AppError::UnprocessableEntity("timestamp is more than 24h in the future".into()));
        }
        Ok(())
    };
    if let Some(a) = next_arrive.as_deref() { skew_check(a)?; }
    if let Some(d) = next_depart.as_deref() { skew_check(d)?; }

    let is_last = stop_idx + 1 == trip.stops.len();
    let should_deliver = is_last
        && next_depart.is_some()
        && matches!(trip.status, crate::models::TripStatus::InTransit);

    state.db.update_trip_stop(id, seq, next_arrive.clone(), next_depart.clone()).await?;
    if should_deliver {
        state.db.transition_trip_status(trip.id, crate::models::TripStatus::Delivered).await?;
    }

    // Re-fetch (AGENTS.md line 325 — chained mutations always re-fetch before returning).
    let final_trip = state.db.get_trip(id).await?;
    let stop = final_trip.stops.iter().find(|s| s.sequence == seq).ok_or(AppError::NotFound)?;

    Ok(Json(serde_json::json!({
        "sequence": stop.sequence,
        "actual_arrive": stop.actual_arrive,
        "actual_depart": stop.actual_depart,
        "timezone": stop.timezone,
        "trip_status": final_trip.status.as_str(),
    })))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn resolve_stop_name_sync(
    fac_map: &HashMap<Uuid, crate::models::FacilityRecord>,
    stop: Option<&crate::models::TripStop>,
) -> String {
    let Some(stop) = stop else { return String::new() };
    if let Some(fid) = stop.facility_id {
        if let Some(f) = fac_map.get(&fid) {
            return f.name.clone();
        }
    }
    stop.name.clone().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{TripStatus, TripStop, TripStopType};
    use chrono::{Duration, Utc};
    use uuid::Uuid;

    fn make_trip(status: TripStatus, stops: Vec<TripStop>) -> TripListItem {
        TripListItem {
            id: Uuid::new_v4(),
            trip_number: "T-001".into(),
            load_id: None,
            load_number: None,
            previous_trip_id: None,
            deadhead_miles: None,
            loaded_miles: None,
            sequence: 0,
            driver_id: None,
            truck_id: None,
            trailer_ids: vec![],
            status,
            stops,
            notes: None,
            owner_id: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            score: None,
        }
    }

    fn stop_with_arrive(arrive: Option<String>) -> TripStop {
        TripStop {
            sequence: 0,
            stop_type: TripStopType::Origin,
            facility_id: None,
            name: None,
            address: None,
            load_stop_index: None,
            scheduled_arrive: arrive,
            scheduled_arrive_end: None,
            actual_arrive: None,
            actual_depart: None,
            expected_dwell_minutes: None,
            detention_free_minutes: None,
            detention_grace_minutes: None,
            notes: None,
            timezone: None,
        }
    }

    #[test]
    fn test_classify_delivered_is_past() {
        let trip = make_trip(TripStatus::Delivered, vec![]);
        assert!(matches!(classify_trip(&trip), TripTab::Past));
    }

    #[test]
    fn test_classify_dispatched_is_current() {
        let trip = make_trip(TripStatus::Dispatched, vec![]);
        assert!(matches!(classify_trip(&trip), TripTab::Current));
    }

    #[test]
    fn test_classify_assigned_future_stop_is_upcoming() {
        let future = (Utc::now() + Duration::hours(1)).to_rfc3339();
        let trip = make_trip(TripStatus::Assigned, vec![stop_with_arrive(Some(future))]);
        assert!(matches!(classify_trip(&trip), TripTab::Upcoming));
    }

    #[test]
    fn test_classify_assigned_past_stop_is_current() {
        let past = (Utc::now() - Duration::hours(1)).to_rfc3339();
        let trip = make_trip(TripStatus::Assigned, vec![stop_with_arrive(Some(past))]);
        assert!(matches!(classify_trip(&trip), TripTab::Current));
    }

    #[test]
    fn test_classify_assigned_no_stop_is_upcoming() {
        let trip = make_trip(TripStatus::Assigned, vec![]);
        assert!(matches!(classify_trip(&trip), TripTab::Upcoming));
    }

    #[test]
    fn test_classify_assigned_unparseable_schedule_is_upcoming() {
        let trip = make_trip(TripStatus::Assigned, vec![stop_with_arrive(Some("not-a-date".into()))]);
        assert!(matches!(classify_trip(&trip), TripTab::Upcoming));
    }

    #[test]
    fn test_classify_assigned_naive_tz_past_scheduled_is_current() {
        // Canonical naive-tz format: scheduled_arrive has no Z/offset, timezone is set.
        // Without parse_stop_time, the RFC3339-only parser failed and this fell through
        // to Upcoming. With parse_stop_time it correctly classifies as Current.
        let tz: chrono_tz::Tz = "America/New_York".parse().unwrap();
        let past_naive = (Utc::now() - Duration::hours(1))
            .with_timezone(&tz)
            .naive_local()
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string();
        let mut stop = stop_with_arrive(Some(past_naive));
        stop.timezone = Some("America/New_York".into());
        let trip = make_trip(TripStatus::Assigned, vec![stop]);
        assert!(matches!(classify_trip(&trip), TripTab::Current));
    }

    #[test]
    fn test_classify_planned_no_stops_is_upcoming() {
        let trip = make_trip(TripStatus::Planned, vec![]);
        assert!(matches!(classify_trip(&trip), TripTab::Upcoming));
    }

    fn make_stop(seq: u32, actual_depart: Option<&str>, tz: Option<&str>) -> TripStop {
        TripStop {
            sequence: seq,
            stop_type: TripStopType::Pickup,
            name: Some("X".into()),
            facility_id: None,
            address: None,
            load_stop_index: None,
            scheduled_arrive: Some("2026-05-01T08:00:00".into()),
            scheduled_arrive_end: None,
            actual_arrive: None,
            actual_depart: actual_depart.map(String::from),
            expected_dwell_minutes: None,
            detention_free_minutes: None,
            detention_grace_minutes: None,
            notes: None,
            timezone: tz.map(String::from),
        }
    }

    #[test]
    fn test_trip_anchor_uses_last_stop_actual_depart() {
        let trip = make_trip(
            TripStatus::Delivered,
            vec![
                make_stop(1, None, Some("America/Los_Angeles")),
                make_stop(2, Some("2026-05-09T23:00:00"), Some("America/Los_Angeles")),
            ],
        );
        let utc = trip_anchor_utc(&trip).unwrap();
        let eastern: chrono_tz::Tz = "America/New_York".parse().unwrap();
        let local = utc.with_timezone(&eastern);
        assert_eq!(local.date_naive(), chrono::NaiveDate::from_ymd_opt(2026, 5, 10).unwrap());
    }

    #[test]
    fn test_sunday_of_a_wednesday() {
        let d = chrono::NaiveDate::from_ymd_opt(2026, 5, 13).unwrap();
        assert_eq!(sunday_of(d), chrono::NaiveDate::from_ymd_opt(2026, 5, 10).unwrap());
    }
}
