// src/api/trips.rs
use crate::{
    ai::embed::embed_text,
    error::AppError,
    models::{
        load::StopType,
        trip::{CreateTripRequest, TripListResponse, TripRecord, TripStatus, TripStop, TripStopType, UpdateTripRequest},
    },
    routing::RoutingClient,
    AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
    Router,
    routing::{delete, get, patch, post},
};
use axum_extra::extract::Query;
use chrono::Utc;
use serde::Deserialize;
use std::collections::HashMap;
use utoipa::IntoParams;
use uuid::Uuid;

#[derive(Deserialize, Default, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListTripsQuery {
    pub load_id: Option<Uuid>,
    pub driver_id: Option<Uuid>,
    pub status: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[utoipa::path(
    post,
    path = "/api/v1/trips",
    request_body(content = CreateTripRequest, description = "Trip to create"),
    responses(
        (status = 201, description = "Created trip record", body = TripRecord),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "trips"
)]
pub async fn create_trip(
    State(state): State<AppState>,
    Json(body): Json<CreateTripRequest>,
) -> Result<impl IntoResponse, AppError> {
    let now = Utc::now();
    use chrono::Datelike;

    let trip_number = match body.trip_number {
        Some(n) => n,
        None => state.db.next_trip_number(now.year()).await?,
    };

    // Fetch load once — used for stop derivation, loaded_miles, and load_number
    let load = if let Some(load_id) = body.load_id {
        Some(state.db.get_load_by_id(load_id).await?)
    } else {
        None
    };

    // Derive or use stops from body
    let raw_stops: Vec<TripStop> = if body.stops.is_empty() {
        if let Some(ref load) = load {
            load.stops.iter().enumerate().map(|(idx, s)| TripStop {
                sequence: s.sequence,
                stop_type: match s.stop_type {
                    StopType::Pickup => TripStopType::Pickup,
                    StopType::Delivery => TripStopType::Delivery,
                },
                facility_id: Some(s.facility_id),
                name: None,
                address: None,
                load_stop_index: Some(idx as u32),
                scheduled_arrive: Some(s.scheduled_arrive.clone()),
                scheduled_arrive_end: s.scheduled_arrive_end.clone(),
                actual_arrive: None,
                actual_depart: None,
                expected_dwell_minutes: s.expected_dwell_minutes,
                detention_free_minutes: s.detention_free_minutes,
                detention_grace_minutes: s.detention_grace_minutes,
                notes: s.notes.clone(),
                timezone: s.timezone.clone(),
                actual_arrive_utc: None,
                actual_depart_utc: None,
            }).collect()
        } else {
            vec![]
        }
    } else {
        body.stops
    };

    // Enrich stops: batch-fetch facilities to populate name + address
    let facility_ids: Vec<Uuid> = raw_stops.iter().filter_map(|s| s.facility_id).collect();
    let facilities = if !facility_ids.is_empty() {
        state.db.batch_get_facilities(&facility_ids).await.unwrap_or_default()
    } else {
        HashMap::new()
    };
    let stops: Vec<TripStop> = raw_stops.into_iter().map(|mut s| {
        if let Some(fac_id) = s.facility_id {
            if let Some(fac) = facilities.get(&fac_id) {
                if s.name.is_none() { s.name = Some(fac.name.clone()); }
                if s.address.is_none() { s.address = Some(fac.address.clone()); }
            }
        }
        s
    }).collect();

    // Resolve previous_trip_id: caller-provided beats auto-lookup
    let previous_trip_id = match body.previous_trip_id {
        Some(id) => Some(id),
        None => match body.driver_id {
            Some(driver_id) => state.db
                .get_last_trip_for_driver(driver_id).await
                .ok()
                .flatten()
                .map(|t| t.id),
            None => None,
        },
    };

    // Compute deadhead + loaded + total + per-leg via a single ORS call.
    let mileage = compute_trip_mileage(&state.db, &state.ors, previous_trip_id, &stops).await;

    // Denormalize load_number
    let load_number = load.as_ref().map(|l| l.load_number.clone());

    let mut record = TripRecord {
        id: Uuid::new_v4(),
        trip_number,
        load_id: body.load_id,
        load_number,
        previous_trip_id,
        deadhead_miles: mileage.deadhead_miles,
        loaded_miles: mileage.loaded_miles,
        total_miles: mileage.total_miles,
        segment_miles: mileage.segment_miles.clone(),
        sequence: body.sequence.unwrap_or(0),
        driver_id: body.driver_id,
        truck_id: body.truck_id,
        trailer_ids: body.trailer_ids,
        status: TripStatus::Planned,
        stops,
        notes: body.notes,
        embedding: None,
        owner_id: 0,
        created_at: now,
        updated_at: now,
    };

    record.embedding = embed_text(&state.ai, &record.embedding_text()).await.ok();

    state.db.insert_trip(&record).await?;
    for s in &mut record.stops { s.fill_utc_fields(); }
    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(
    get,
    path = "/api/v1/trips",
    params(ListTripsQuery),
    responses(
        (status = 200, description = "List of trips", body = TripListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "trips"
)]
pub async fn list_trips(
    State(state): State<AppState>,
    Query(q): Query<ListTripsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let _limit = q.limit.unwrap_or(20).min(100);
    let _offset = q.offset.unwrap_or(0);

    let mut items = state.db.list_trips(
        q.load_id, q.driver_id, q.status.as_deref(),
    ).await?;
    for it in &mut items {
        for s in &mut it.stops { s.fill_utc_fields(); }
    }
    let returned = items.len();
    Ok(Json(TripListResponse { returned, items }))
}

#[utoipa::path(
    get,
    path = "/api/v1/trips/{id}",
    params(
        ("id" = Uuid, Path, description = "Trip UUID")
    ),
    responses(
        (status = 200, description = "Trip record", body = TripRecord),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "trips"
)]
pub async fn get_trip(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let mut record = state.db.get_trip(id).await?;
    for s in &mut record.stops { s.fill_utc_fields(); }
    Ok(Json(record))
}

#[utoipa::path(
    patch,
    path = "/api/v1/trips/{id}",
    params(
        ("id" = Uuid, Path, description = "Trip UUID")
    ),
    request_body(content = UpdateTripRequest, description = "Fields to update — all optional"),
    responses(
        (status = 200, description = "Updated trip record", body = TripRecord),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "trips"
)]
pub async fn update_trip(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateTripRequest>,
) -> Result<impl IntoResponse, AppError> {
    let existing = state.db.get_trip(id).await?;

    let embed_stops = body.stops.as_ref().unwrap_or(&existing.stops);
    let stop_names = embed_stops.iter()
        .filter_map(|s| s.name.as_deref())
        .collect::<Vec<_>>().join(" ");
    let trip_number = &existing.trip_number;
    let notes_str = body.notes.as_deref()
        .or(existing.notes.as_deref())
        .unwrap_or("");
    let embed_text_str = format!("{trip_number} {stop_names} {notes_str}");
    let embedding = embed_text(&state.ai, &embed_text_str).await.ok();

    let mut record = state.db.update_trip_metadata(
        id, body.load_id, body.sequence, body.stops, body.notes, embedding,
    ).await?;
    for s in &mut record.stops { s.fill_utc_fields(); }
    Ok(Json(record))
}

#[utoipa::path(
    delete,
    path = "/api/v1/trips/{id}",
    params(
        ("id" = Uuid, Path, description = "Trip UUID")
    ),
    responses(
        (status = 204, description = "Trip was active → soft-cancelled (status set to Cancelled); or trip was already Cancelled → hard-deleted (row removed)"),
        (status = 409, description = "Cannot cancel in_transit, delivered, or completed trip"),
        (status = 404, description = "Trip not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "trips"
)]
pub async fn delete_trip(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    state.db.delete_trip(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/trips", post(create_trip))
        .route("/api/v1/trips", get(list_trips))
        .route("/api/v1/trips/:id", get(get_trip))
        .route("/api/v1/trips/:id", patch(update_trip))
        .route("/api/v1/trips/:id", delete(delete_trip))
}

struct ComputedMileage {
    deadhead_miles: Option<f64>,
    loaded_miles: Option<f64>,
    total_miles: Option<f64>,
    /// Per-segment miles in the order [deadhead, loaded_legs...] when deadhead exists,
    /// or just [loaded_legs...] when there's no previous trip.
    segment_miles: Vec<f64>,
    /// Resolved previous-trip last facility (deadhead origin), if available.
    deadhead_origin_facility_id: Option<Uuid>,
}

async fn compute_trip_mileage(
    db: &crate::db::DbClient,
    ors: &RoutingClient,
    previous_trip_id: Option<Uuid>,
    trip_stops: &[TripStop],
) -> ComputedMileage {
    let mut empty = ComputedMileage {
        deadhead_miles: None, loaded_miles: None, total_miles: None,
        segment_miles: vec![], deadhead_origin_facility_id: None,
    };
    if trip_stops.is_empty() { return empty; }

    // Resolve deadhead origin facility if a previous trip exists.
    let deadhead_origin_fac: Option<Uuid> = match previous_trip_id {
        Some(prev_id) => db.get_trip(prev_id).await.ok()
            .and_then(|t| t.stops.last().and_then(|s| s.facility_id)),
        None => None,
    };
    empty.deadhead_origin_facility_id = deadhead_origin_fac;

    // Build the waypoint list: [deadhead_origin?, stop_0, stop_1, ...]
    let mut fac_ids: Vec<Uuid> = Vec::new();
    if let Some(fid) = deadhead_origin_fac { fac_ids.push(fid); }
    for s in trip_stops {
        match s.facility_id {
            Some(fid) => fac_ids.push(fid),
            None => return empty,
        }
    }
    if fac_ids.len() < 2 { return empty; }

    let facilities = match db.batch_get_facilities(&fac_ids).await {
        Ok(f) => f,
        Err(_) => return empty,
    };

    let mut waypoints: Vec<(f64, f64)> = Vec::with_capacity(fac_ids.len());
    for fid in &fac_ids {
        let f = match facilities.get(fid) { Some(f) => f, None => return empty };
        let (lat, lng) = match (f.lat, f.lng) { (Some(la), Some(lo)) => (la, lo), _ => return empty };
        waypoints.push((lat, lng));
    }

    let route = match ors.calculate_route_with_segments(&waypoints).await {
        Some(r) => r,
        None => return empty,
    };

    let has_deadhead = deadhead_origin_fac.is_some();
    let (deadhead, loaded_segs): (Option<f64>, &[f64]) = if has_deadhead {
        match route.segment_miles.split_first() {
            Some((first, rest)) => (Some(*first), rest),
            None => return empty,
        }
    } else {
        (None, &route.segment_miles[..])
    };
    let loaded: Option<f64> = if loaded_segs.is_empty() {
        None
    } else {
        Some(loaded_segs.iter().sum())
    };
    let total = Some(route.total_miles);

    ComputedMileage {
        deadhead_miles: deadhead,
        loaded_miles: loaded,
        total_miles: total,
        segment_miles: route.segment_miles,
        deadhead_origin_facility_id: deadhead_origin_fac,
    }
}

/// Recomputes mileage for an existing trip and persists the result via `merge_insert`.
/// Loads the trip, resolves the previous trip's last facility + the trip's own stop
/// facilities to coordinates, calls ORS, and writes `deadhead_miles`, `loaded_miles`,
/// `total_miles`, `segment_miles`. Returns the freshly built `MileageSummary`.
///
/// Returns `AppError::OrsRoutingUnavailable` (409) when ORS returns no route OR a
/// required facility has no lat/lng. Returns `AppError::NotFound` when the trip
/// does not exist.
pub async fn compute_and_persist_mileage(
    state: &crate::AppState,
    trip_id: Uuid,
) -> Result<crate::models::trip::MileageSummary, AppError> {
    let trip = state.db.get_trip(trip_id).await?;
    let computed = compute_trip_mileage(&state.db, &state.ors, trip.previous_trip_id, &trip.stops).await;

    // Detect failure: if a previous trip exists OR stops exist with potential routing,
    // but we got no segment data back, ORS or coords were unavailable.
    let expected_segments = trip.stops.len()
        + usize::from(trip.previous_trip_id.is_some());
    let have_route = !computed.segment_miles.is_empty()
        && computed.total_miles.is_some();

    if !have_route && expected_segments >= 2 {
        return Err(AppError::OrsRoutingUnavailable(
            "could not resolve route — ORS unavailable or facility coordinates missing".into(),
        ));
    }

    let updated = state.db.update_trip_mileage(
        trip_id,
        computed.deadhead_miles,
        computed.loaded_miles,
        computed.total_miles,
        computed.segment_miles,
    ).await?;

    Ok(crate::api::mileage_summary::build_mileage_summary(state, &updated).await)
}
