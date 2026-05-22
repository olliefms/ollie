// src/api/dispatcher_portal/data.rs
//
// Dispatcher portal data endpoints — protected by require_dispatcher_jwt.
// All business logic is delegated to the same DB ops used by the admin API.
// No new DTOs are introduced; admin response shapes are reused throughout.

use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    Json,
};
use axum::http::StatusCode;
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    models::{
        DriverListResponse, DriverStatus,
        LoadDetailResponse,
        TrailerListResponse,
        TruckListResponse,
        EventListResponse, EventResponse,
        LoadStatus, TrailerStatus, TripStatus, TruckStatus,
    },
    api::loads::{ListLoadsQuery, resolve_stops_pub},
    events,
};
use crate::models::{CreateLoadRequest, UpdateLoadRequest};
use crate::api::trip_actions::{
    self, AssignTripRequest, CheckCallRequest, StopArriveRequest, StopDepartRequest,
    StopLateRequest,
};

#[derive(serde::Serialize)]
pub struct DispatcherTripListItem {
    pub id: uuid::Uuid,
    pub trip_number: String,
    pub status: String,
    pub driver_id: Option<uuid::Uuid>,
    pub driver_name: Option<String>,
    pub truck_id: Option<uuid::Uuid>,
    pub truck_unit: Option<String>,
    pub trailer_ids: Vec<uuid::Uuid>,
    pub trailer_units: Vec<String>,
    pub stops: Vec<crate::models::TripStop>,
    pub load_id: Option<uuid::Uuid>,
    pub load_number: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mileage_summary: Option<crate::models::trip::MileageSummary>,
}

#[derive(serde::Serialize)]
pub struct DispatcherTripListResponse {
    pub items: Vec<DispatcherTripListItem>,
}

#[derive(serde::Serialize)]
pub struct DispatchStopSummary {
    pub name: Option<String>,
    pub scheduled_arrive: String,
}

#[derive(serde::Serialize)]
pub struct DispatchLoadListItem {
    pub id: uuid::Uuid,
    pub load_number: String,
    pub status: String,
    pub customer_name: String,
    pub stops: Vec<DispatchStopSummary>,
    pub score: Option<f32>,
}

#[derive(serde::Serialize)]
pub struct DispatchLoadListResponse {
    pub returned: usize,
    pub items: Vec<DispatchLoadListItem>,
}

async fn enrich_loads(
    state: &crate::AppState,
    items: Vec<crate::models::LoadListItem>,
) -> Vec<DispatchLoadListItem> {
    use std::collections::HashSet;
    let all_fac_ids: Vec<uuid::Uuid> = items.iter()
        .flat_map(|item| item.stops.iter().map(|s| s.facility_id))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let fac_map = state.db.batch_get_facilities(&all_fac_ids).await.unwrap_or_default();

    items.into_iter().map(|item| {
        let stops = item.stops.iter().map(|s| DispatchStopSummary {
            name: fac_map.get(&s.facility_id).map(|f| f.name.clone()),
            scheduled_arrive: s.scheduled_arrive.clone(),
        }).collect();
        DispatchLoadListItem {
            id: item.id,
            load_number: item.load_number,
            status: item.status.as_str().to_string(),
            customer_name: item.customer_name,
            stops,
            score: item.score,
        }
    }).collect()
}

fn enrich_trip(
    trip: crate::models::TripListItem,
    driver_map: &std::collections::HashMap<uuid::Uuid, String>,
    truck_map: &std::collections::HashMap<uuid::Uuid, String>,
    trailer_map: &std::collections::HashMap<uuid::Uuid, String>,
) -> DispatcherTripListItem {
    let driver_name = trip.driver_id.and_then(|did| driver_map.get(&did).cloned());
    let truck_unit  = trip.truck_id.and_then(|tid| truck_map.get(&tid).cloned());
    let trailer_units = trip.trailer_ids.iter()
        .filter_map(|tid| trailer_map.get(tid).cloned())
        .collect();

    let mut stops = trip.stops;
    for s in &mut stops { s.fill_utc_fields(); }

    DispatcherTripListItem {
        id: trip.id,
        trip_number: trip.trip_number,
        status: trip.status.as_str().to_string(),
        driver_id: trip.driver_id,
        driver_name,
        truck_id: trip.truck_id,
        truck_unit,
        trailer_ids: trip.trailer_ids,
        trailer_units,
        stops,
        load_id: trip.load_id,
        load_number: trip.load_number,
        created_at: trip.created_at,
        updated_at: trip.updated_at,
        mileage_summary: None,
    }
}

// ---------------------------------------------------------------------------
// Loads
// ---------------------------------------------------------------------------

#[utoipa::path(
    get,
    path = "/dispatch/api/v1/loads",
    params(
        ("status" = Option<String>, Query, description = "Filter by status"),
        ("facility_id" = Option<Uuid>, Query, description = "Filter by facility ID"),
        ("limit" = Option<usize>, Query, description = "Max results (default 20, max 100)"),
        ("offset" = Option<usize>, Query, description = "Pagination offset"),
    ),
    responses(
        (status = 200, description = "List of loads", body = DispatchLoadListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn list_loads(
    State(state): State<AppState>,
    Query(q): Query<ListLoadsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let limit = q.limit.unwrap_or(20).min(100);
    let offset = q.offset.unwrap_or(0);

    let (total, items) = state.db.list_loads(
        q.status.as_deref(),
        q.customer.as_deref(),
        &q.tag,
        q.from.as_deref(),
        q.to.as_deref(),
        limit,
        offset,
    ).await?;
    let enriched = enrich_loads(&state, items).await;
    Ok(Json(DispatchLoadListResponse { returned: total, items: enriched }))
}

#[utoipa::path(
    get,
    path = "/dispatch/api/v1/loads/{id}",
    params(("id" = Uuid, Path, description = "Load UUID")),
    responses(
        (status = 200, description = "Load detail", body = LoadDetailResponse),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn get_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_load_by_id(id).await?;
    let response = build_load_detail(&state, record).await?;
    Ok(Json(response))
}

#[utoipa::path(
    post,
    path = "/dispatch/api/v1/loads",
    request_body(content = CreateLoadRequest, description = "Load to create"),
    responses(
        (status = 201, description = "Created load", body = LoadDetailResponse),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn create_load(
    State(state): State<AppState>,
    Json(body): Json<CreateLoadRequest>,
) -> Result<impl IntoResponse, AppError> {
    use chrono::Utc;
    use crate::models::{LoadRecord, LoadStatus};

    let stops = resolve_stops_pub(&state, body.stops).await?;
    let now = Utc::now();

    let load_number = match body.load_number {
        Some(n) => n,
        None => {
            use chrono::Datelike;
            state.db.next_load_number(now.year()).await?
        }
    };

    let facility_ids: Vec<Uuid> = stops.iter().map(|s| s.facility_id).collect();
    let facilities = state.db.batch_get_facilities(&facility_ids).await?;
    let stop_text = stops.iter()
        .filter_map(|s| facilities.get(&s.facility_id))
        .map(|f| format!("{} {}", f.name, f.address))
        .collect::<Vec<_>>()
        .join(" ");
    let embed_text_str = format!(
        "{} {} {} {} {}",
        body.customer_name,
        stop_text,
        body.commodity.as_deref().unwrap_or(""),
        body.notes.as_deref().unwrap_or(""),
        body.tags.join(" "),
    );
    let embedding = crate::ai::embed::embed_text(&state.ai, &embed_text_str).await.ok();

    let record = LoadRecord {
        id: Uuid::new_v4(),
        load_number,
        owner_id: 0,
        status: LoadStatus::Planned,
        customer_name: body.customer_name,
        customer_ref: body.customer_ref,
        stops,
        rate_items: body.rate_items,
        commodity: body.commodity,
        weight_lbs: body.weight_lbs,
        miles: body.miles,
        notes: body.notes,
        tags: body.tags,
        blob_ids: body.blob_ids,
        invoice_number: None,
        invoice_date: None,
        cancellation_reason: None,
        embedding,
        created_at: now,
        updated_at: now,
    };

    state.db.insert_load(&record).await?;

    if record.miles.is_none() {
        let _ = state.routing_tx.try_send(record.id);
    }

    let response = build_load_detail(&state, record).await?;
    Ok((StatusCode::CREATED, Json(response)))
}

#[utoipa::path(
    put,
    path = "/dispatch/api/v1/loads/{id}",
    params(("id" = Uuid, Path, description = "Load UUID")),
    request_body(content = UpdateLoadRequest, description = "Fields to update"),
    responses(
        (status = 200, description = "Updated load", body = LoadDetailResponse),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn update_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateLoadRequest>,
) -> Result<impl IntoResponse, AppError> {
    let stops_provided = body.stops.is_some();
    let stops = match body.stops {
        Some(inputs) => Some(resolve_stops_pub(&state, inputs).await?),
        None => None,
    };

    let existing = state.db.get_load_by_id(id).await?;
    let effective_stops = stops.as_ref().unwrap_or(&existing.stops);
    let facility_ids: Vec<Uuid> = effective_stops.iter().map(|s| s.facility_id).collect();
    let facilities = state.db.batch_get_facilities(&facility_ids).await?;
    let stop_text = effective_stops.iter()
        .filter_map(|s| facilities.get(&s.facility_id))
        .map(|f| format!("{} {}", f.name, f.address))
        .collect::<Vec<_>>()
        .join(" ");
    let embed_text_str = format!(
        "{} {} {} {} {}",
        body.customer_name.as_deref().unwrap_or(&existing.customer_name),
        stop_text,
        body.commodity.as_deref().unwrap_or(existing.commodity.as_deref().unwrap_or("")),
        body.notes.as_deref().unwrap_or(existing.notes.as_deref().unwrap_or("")),
        body.tags.as_ref().unwrap_or(&existing.tags).join(" "),
    );
    let embedding = crate::ai::embed::embed_text(&state.ai, &embed_text_str).await.ok();

    let mut updated = state.db.update_load_metadata(
        id,
        body.customer_name,
        body.customer_ref,
        stops,
        body.rate_items,
        body.commodity,
        body.weight_lbs,
        body.miles,
        body.notes,
        body.tags,
        body.blob_ids,
        embedding,
    ).await?;

    if stops_provided && body.miles.is_none() {
        state.db.clear_load_miles(id).await?;
        updated.miles = None;
        let _ = state.routing_tx.try_send(id);
    }

    let response = build_load_detail(&state, updated).await?;
    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// Trips
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListTripsQuery {
    pub load_id: Option<Uuid>,
    pub driver_id: Option<Uuid>,
    pub status: Option<String>,
}

#[utoipa::path(
    get,
    path = "/dispatch/api/v1/trips",
    params(
        ("load_id" = Option<Uuid>, Query, description = "Filter by load ID"),
        ("driver_id" = Option<Uuid>, Query, description = "Filter by driver ID"),
        ("status" = Option<String>, Query, description = "Filter by status"),
    ),
    responses(
        (status = 200, description = "List of trips (enriched with driver/truck names)"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn list_trips(
    State(state): State<AppState>,
    Query(q): Query<ListTripsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let trips = state.db.list_trips(q.load_id, q.driver_id, q.status.as_deref()).await?;

    let mut driver_ids = std::collections::HashSet::new();
    let mut truck_ids  = std::collections::HashSet::new();
    let mut trailer_ids_all = std::collections::HashSet::new();
    for trip in &trips {
        if let Some(did) = trip.driver_id { driver_ids.insert(did); }
        if let Some(tid) = trip.truck_id  { truck_ids.insert(tid); }
        for tid in &trip.trailer_ids { trailer_ids_all.insert(*tid); }
    }

    let driver_ids_vec:  Vec<_> = driver_ids.into_iter().collect();
    let truck_ids_vec:   Vec<_> = truck_ids.into_iter().collect();
    let trailer_ids_vec: Vec<_> = trailer_ids_all.into_iter().collect();

    let (driver_records, truck_records, trailer_records) = tokio::try_join!(
        state.db.batch_get_drivers(&driver_ids_vec),
        state.db.batch_get_trucks(&truck_ids_vec),
        state.db.batch_get_trailers(&trailer_ids_vec),
    )?;

    let driver_map:  std::collections::HashMap<_, _> = driver_records.into_iter().map(|(id, r)| (id, r.name)).collect();
    let truck_map:   std::collections::HashMap<_, _> = truck_records.into_iter().map(|(id, r)| (id, r.unit_number)).collect();
    let trailer_map: std::collections::HashMap<_, _> = trailer_records.into_iter().map(|(id, r)| (id, r.unit_number)).collect();

    let items: Vec<DispatcherTripListItem> = trips.into_iter()
        .map(|trip| enrich_trip(trip, &driver_map, &truck_map, &trailer_map))
        .collect();

    Ok(Json(DispatcherTripListResponse { items }))
}

#[utoipa::path(
    get,
    path = "/dispatch/api/v1/trips/{id}",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    responses(
        (status = 200, description = "Trip record (enriched with driver/truck names)"),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn get_trip(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_trip(id).await?;
    let trip: crate::models::TripListItem = record.clone().into();

    let driver_ids_vec:  Vec<_> = trip.driver_id.into_iter().collect();
    let truck_ids_vec:   Vec<_> = trip.truck_id.into_iter().collect();
    let trailer_ids_vec: Vec<_> = trip.trailer_ids.clone();

    let (driver_records, truck_records, trailer_records) = tokio::try_join!(
        state.db.batch_get_drivers(&driver_ids_vec),
        state.db.batch_get_trucks(&truck_ids_vec),
        state.db.batch_get_trailers(&trailer_ids_vec),
    )?;

    let driver_map:  std::collections::HashMap<_, _> = driver_records.into_iter().map(|(id, r)| (id, r.name)).collect();
    let truck_map:   std::collections::HashMap<_, _> = truck_records.into_iter().map(|(id, r)| (id, r.unit_number)).collect();
    let trailer_map: std::collections::HashMap<_, _> = trailer_records.into_iter().map(|(id, r)| (id, r.unit_number)).collect();

    let mut enriched = enrich_trip(trip, &driver_map, &truck_map, &trailer_map);
    enriched.mileage_summary = Some(
        crate::api::mileage_summary::build_mileage_summary(&state, &record).await
    );
    Ok(Json(enriched))
}

#[utoipa::path(
    post,
    path = "/dispatch/api/v1/trips/{id}/assign",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    request_body(content = AssignTripRequest, description = "Driver, truck, and optional trailers"),
    responses(
        (status = 200, description = "Trip assigned", body = TripRecord),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — driver/truck/trailer not eligible for assignment (inactive/out-of-service) or invalid status transition"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn assign_trip(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<AssignTripRequest>,
) -> Result<impl IntoResponse, AppError> {
    use crate::models::DriverStatus;

    let driver = state.db.get_driver_by_id(body.driver_id).await?;
    if driver.status == DriverStatus::Inactive {
        return Err(AppError::Conflict(format!("driver {} is not available for assignment", body.driver_id)));
    }

    let truck = state.db.get_truck_by_id(body.truck_id).await?;
    if matches!(truck.status, TruckStatus::OutOfService | TruckStatus::Inactive) {
        return Err(AppError::Conflict(format!("truck {} is not available for assignment", body.truck_id)));
    }

    // Pre-validate all trailers before any mutation to prevent partial state
    let mut trailers = Vec::new();
    for &trailer_id in &body.trailer_ids {
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

    state.db.transition_trip_status(id, TripStatus::Assigned).await?;
    state
        .db
        .update_trip_resources(id, Some(body.driver_id), Some(body.truck_id), body.trailer_ids.clone())
        .await?;

    if driver.status == DriverStatus::Available {
        state.db.update_driver_status(body.driver_id, DriverStatus::Assigned).await?;
    }
    if truck.status == TruckStatus::Available {
        state.db.update_truck_status(body.truck_id, TruckStatus::Assigned).await?;
    }
    for trailer in &trailers {
        if trailer.status == TrailerStatus::Available {
            state.db.update_trailer_status(trailer.id, TrailerStatus::Assigned).await?;
        }
    }

    // Re-fetch after all mutations (stale-return rule)
    let trip = state.db.get_trip(id).await?;

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
    path = "/dispatch/api/v1/trips/{id}/unassign",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    responses(
        (status = 200, description = "Trip unassigned", body = TripRecord),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — invalid status transition"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn unassign_trip(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let existing = state.db.get_trip(id).await?;
    state.db.transition_trip_status(id, TripStatus::Planned).await?;
    state.db.update_trip_resources(id, None, None, vec![]).await?;

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

    // Re-fetch after all mutations (stale-return rule)
    let trip = state.db.get_trip(id).await?;
    events::on_trip_unassigned(&state.db, id).await;
    Ok(Json(trip))
}

// ---------------------------------------------------------------------------
// Trip lifecycle actions — thin wrappers around admin trip_actions handlers.
// Same business logic; separate utoipa annotations to surface under the
// dispatcher path/tag.
// ---------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/dispatch/api/v1/trips/{id}/dispatch",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    responses(
        (status = 200, description = "Trip dispatched", body = TripRecord),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — trip must be in assigned status"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn dispatch_trip(
    state: State<AppState>,
    id: Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    trip_actions::dispatch_trip(state, id).await
}

#[utoipa::path(
    post,
    path = "/dispatch/api/v1/trips/{id}/undispatch",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    responses(
        (status = 200, description = "Trip undispatched back to assigned", body = TripRecord),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — trip must be in dispatched status (not in_transit or beyond)"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn undispatch_trip(
    state: State<AppState>,
    id: Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    trip_actions::undispatch_trip(state, id).await
}

#[utoipa::path(
    post,
    path = "/dispatch/api/v1/trips/{id}/cancel",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    responses(
        (status = 200, description = "Trip cancelled", body = TripRecord),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — cannot cancel a trip that is in_transit or delivered"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn cancel_trip(
    state: State<AppState>,
    id: Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    trip_actions::cancel_trip(state, id).await
}

#[utoipa::path(
    post,
    path = "/dispatch/api/v1/trips/{id}/complete",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    responses(
        (status = 204, description = "Trip completed and resources released"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — trip must be in delivered status"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn complete_trip(
    state: State<AppState>,
    id: Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    trip_actions::complete_trip(state, id).await
}

#[utoipa::path(
    post,
    path = "/dispatch/api/v1/trips/{id}/stops/{seq}/arrive",
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
    tag = "dispatch"
)]
pub async fn stop_arrive(
    state: State<AppState>,
    path: Path<(Uuid, u32)>,
    body: Json<StopArriveRequest>,
) -> Result<impl IntoResponse, AppError> {
    trip_actions::stop_arrive(state, path, body).await
}

#[utoipa::path(
    post,
    path = "/dispatch/api/v1/trips/{id}/stops/{seq}/depart",
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
    tag = "dispatch"
)]
pub async fn stop_depart(
    state: State<AppState>,
    path: Path<(Uuid, u32)>,
    body: Json<StopDepartRequest>,
) -> Result<impl IntoResponse, AppError> {
    trip_actions::stop_depart(state, path, body).await
}

#[utoipa::path(
    post,
    path = "/dispatch/api/v1/trips/{id}/stops/{seq}/late",
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
    tag = "dispatch"
)]
pub async fn stop_late(
    state: State<AppState>,
    path: Path<(Uuid, u32)>,
    body: Json<StopLateRequest>,
) -> Result<impl IntoResponse, AppError> {
    trip_actions::stop_late(state, path, body).await
}

#[utoipa::path(
    post,
    path = "/dispatch/api/v1/trips/{id}/check-call",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    request_body(content = CheckCallRequest, description = "Location, notes, and optional next-stop ETA"),
    responses(
        (status = 204, description = "Check call recorded"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn check_call(
    state: State<AppState>,
    id: Path<Uuid>,
    body: Json<CheckCallRequest>,
) -> Result<impl IntoResponse, AppError> {
    trip_actions::check_call(state, id, body).await
}

// ---------------------------------------------------------------------------
// Drivers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListDriversQuery {
    pub status: Option<String>,
}

#[utoipa::path(
    get,
    path = "/dispatch/api/v1/drivers",
    params(
        ("status" = Option<String>, Query, description = "Filter by status"),
    ),
    responses(
        (status = 200, description = "List of drivers", body = DriverListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn list_drivers(
    State(state): State<AppState>,
    Query(q): Query<ListDriversQuery>,
) -> Result<impl IntoResponse, AppError> {
    let (total, items) = state.db.list_drivers(q.status.as_deref(), 100, 0).await?;
    Ok(Json(DriverListResponse { returned: total, items }))
}

#[utoipa::path(
    get,
    path = "/dispatch/api/v1/drivers/{id}",
    params(("id" = Uuid, Path, description = "Driver UUID")),
    responses(
        (status = 200, description = "Driver record", body = DriverRecord),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn get_driver(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_driver_by_id(id).await?;
    Ok(Json(record))
}

// ---------------------------------------------------------------------------
// Trucks
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListTrucksQuery {
    pub status: Option<String>,
}

#[utoipa::path(
    get,
    path = "/dispatch/api/v1/trucks",
    params(
        ("status" = Option<String>, Query, description = "Filter by status"),
    ),
    responses(
        (status = 200, description = "List of trucks", body = TruckListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn list_trucks(
    State(state): State<AppState>,
    Query(q): Query<ListTrucksQuery>,
) -> Result<impl IntoResponse, AppError> {
    let (total, items) = state.db.list_trucks(q.status.as_deref(), 100, 0).await?;
    Ok(Json(TruckListResponse { returned: total, items }))
}

#[utoipa::path(
    get,
    path = "/dispatch/api/v1/trucks/{id}",
    params(("id" = Uuid, Path, description = "Truck UUID")),
    responses(
        (status = 200, description = "Truck record", body = TruckRecord),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn get_truck(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_truck_by_id(id).await?;
    Ok(Json(record))
}

// ---------------------------------------------------------------------------
// Trailers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListTrailersQuery {
    pub status: Option<String>,
}

#[utoipa::path(
    get,
    path = "/dispatch/api/v1/trailers",
    params(
        ("status" = Option<String>, Query, description = "Filter by status"),
    ),
    responses(
        (status = 200, description = "List of trailers", body = TrailerListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn list_trailers(
    State(state): State<AppState>,
    Query(q): Query<ListTrailersQuery>,
) -> Result<impl IntoResponse, AppError> {
    let (total, items) = state.db.list_trailers(q.status.as_deref(), None, 100, 0).await?;
    Ok(Json(TrailerListResponse { returned: total, items }))
}

#[utoipa::path(
    get,
    path = "/dispatch/api/v1/trailers/{id}",
    params(("id" = Uuid, Path, description = "Trailer UUID")),
    responses(
        (status = 200, description = "Trailer record", body = TrailerRecord),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn get_trailer(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_trailer_by_id(id).await?;
    Ok(Json(record))
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListEventsDispatchQuery {
    pub trip_id: Option<Uuid>,
    pub driver_id: Option<Uuid>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[utoipa::path(
    get,
    path = "/dispatch/api/v1/events",
    params(
        ("trip_id" = Option<Uuid>, Query, description = "Filter by trip ID"),
        ("driver_id" = Option<Uuid>, Query, description = "Filter by driver ID"),
        ("limit" = Option<usize>, Query, description = "Max results (default 20, max 100)"),
        ("offset" = Option<usize>, Query, description = "Pagination offset"),
    ),
    responses(
        (status = 200, description = "List of events", body = EventListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn list_events(
    State(state): State<AppState>,
    Query(q): Query<ListEventsDispatchQuery>,
) -> Result<impl IntoResponse, AppError> {
    let limit = q.limit.unwrap_or(20).min(100);
    let offset = q.offset.unwrap_or(0);

    // Use trip_id or driver_id as entity_id filter (trip takes priority)
    let entity_id = q.trip_id.or(q.driver_id);

    let (_total, records) = state.db.query_events(
        entity_id,
        None,
        None,
        None,
        None,
        limit,
        offset,
    ).await?;
    let items: Vec<EventResponse> = records.into_iter().map(EventResponse::from).collect();
    Ok(Json(EventListResponse { returned: items.len(), items }))
}

// ---------------------------------------------------------------------------
// KPI count endpoints — each returns { count: N } for the home dashboard
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
pub struct CountResponse {
    pub count: usize,
}

/// Count loads that are not yet in a terminal state (delivered / invoiced / settled / cancelled).
#[utoipa::path(
    get, path = "/dispatch/api/v1/loads/count",
    responses(
        (status = 200, description = "Open load count"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn count_open_loads(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let filter = Some("status = 'planned' OR status = 'assigned' OR status = 'dispatched' OR status = 'in_transit'".to_string());
    let count = state.db.load_table.count_rows(filter).await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(CountResponse { count }))
}

/// Count drivers with active status.
#[utoipa::path(
    get, path = "/dispatch/api/v1/drivers/count",
    responses(
        (status = 200, description = "Active driver count"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn count_active_drivers(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let filter = Some("status = 'available' OR status = 'assigned' OR status = 'dispatched'".to_string());
    let count = state.db.driver_table.count_rows(filter).await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(CountResponse { count }))
}

/// Count blobs with pending status.
#[utoipa::path(
    get, path = "/dispatch/api/v1/blobs/count",
    responses(
        (status = 200, description = "Pending document count"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn count_pending_documents(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let filter = Some("status = 'pending'".to_string());
    let count = state.db.blob_table.count_rows(filter).await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(CountResponse { count }))
}

/// Count events that occurred today (UTC).
#[utoipa::path(
    get, path = "/dispatch/api/v1/events/count",
    responses(
        (status = 200, description = "Events today count"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn count_events_today(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let today = chrono::Utc::now().date_naive().format("%Y-%m-%dT00:00:00Z").to_string();
    let filter = Some(format!("occurred_at >= '{today}'"));
    let count = state.db.event_table.count_rows(filter).await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(CountResponse { count }))
}

// ---------------------------------------------------------------------------
// Internal helpers — mirror build_detail_response from src/api/loads.rs
// ---------------------------------------------------------------------------

async fn build_load_detail(
    state: &AppState,
    record: crate::models::LoadRecord,
) -> Result<LoadDetailResponse, AppError> {
    use crate::models::{StopResponse, LoadDetailResponse};

    let facility_ids: Vec<Uuid> = record.stops.iter().map(|s| s.facility_id).collect();
    let facilities = state.db.batch_get_facilities(&facility_ids).await?;

    let stops: Vec<StopResponse> = record.stops.iter().map(|stop| {
        let facility = facilities.get(&stop.facility_id);
        let actual_arrive_utc = stop.actual_arrive.as_deref()
            .and_then(|s| crate::models::load::naive_local_to_utc(s, stop.timezone.as_deref()));
        let actual_depart_utc = stop.actual_depart.as_deref()
            .and_then(|s| crate::models::load::naive_local_to_utc(s, stop.timezone.as_deref()));
        StopResponse {
            sequence: stop.sequence,
            stop_type: stop.stop_type.clone(),
            service_type: stop.service_type.clone(),
            facility_id: stop.facility_id,
            facility_name: facility.map(|f| f.name.clone()).unwrap_or_default(),
            address: facility.map(|f| f.address.clone()).unwrap_or_default(),
            normalized_address: facility.and_then(|f| f.normalized_address.clone()),
            lat: facility.and_then(|f| f.lat),
            lng: facility.and_then(|f| f.lng),
            scheduled_arrive: stop.scheduled_arrive.clone(),
            scheduled_arrive_end: stop.scheduled_arrive_end.clone(),
            actual_arrive: stop.actual_arrive.clone(),
            actual_depart: stop.actual_depart.clone(),
            expected_dwell_minutes: stop.expected_dwell_minutes,
            detention_free_minutes: stop.detention_free_minutes,
            detention_grace_minutes: stop.detention_grace_minutes,
            notes: stop.notes.clone(),
            blob_ids: stop.blob_ids.clone(),
            timezone: stop.timezone.clone(),
            actual_arrive_utc,
            actual_depart_utc,
        }
    }).collect();

    let mileage_summary = {
        let mut trips = state.db.list_trips_for_load(record.id).await.unwrap_or_default();
        trips.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        if let Some(trip_record) = trips.into_iter().next() {
            Some(crate::api::mileage_summary::build_mileage_summary(state, &trip_record).await)
        } else {
            None
        }
    };

    let total_rate_usd = record.total_rate_usd();
    Ok(LoadDetailResponse {
        id: record.id,
        load_number: record.load_number,
        status: record.status,
        customer_name: record.customer_name,
        customer_ref: record.customer_ref,
        stops,
        rate_items: record.rate_items,
        total_rate_usd,
        commodity: record.commodity,
        weight_lbs: record.weight_lbs,
        miles: record.miles,
        notes: record.notes,
        tags: record.tags,
        blob_ids: record.blob_ids,
        invoice_number: record.invoice_number,
        invoice_date: record.invoice_date,
        cancellation_reason: record.cancellation_reason,
        created_at: record.created_at,
        updated_at: record.updated_at,
        mileage_summary,
    })
}
