// src/api/fleet_portal/data.rs
//
// Fleet portal data endpoints — protected by require_fleet_user_jwt.
// All business logic is delegated to the same DB ops used by the admin API.
// No new DTOs are introduced; admin response shapes are reused throughout.

use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    Extension, Json,
};
use axum::http::StatusCode;
use super::jwt::FleetUserClaims;
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    models::{
        DriverListResponse,
        LoadDetailResponse,
        MaintenanceListResponse,
        TrailerListResponse,
        TruckListResponse,
        EventListResponse, EventResponse,
        LoadStatus,
    },
    api::loads::{ListLoadsQuery, resolve_stops_pub},
};
use crate::models::{CreateLoadRequest, UpdateLoadRequest};
use crate::services::trip_lifecycle::{
    AssignTripRequest, CheckCallRequest, StopArriveRequest, StopDepartRequest, StopLateRequest,
};

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct FleetTripListItem {
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
    pub notes: Option<String>,
    /// Flattened mileage projection for list views — keeps payloads small but
    /// gives agents enough info to audit the fleet without N+1 `get_trip` calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadhead_miles: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded_miles: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_miles: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_facility_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mileage_summary: Option<crate::models::trip::MileageSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver_pay: Option<crate::models::pay::DriverPay>,
    /// Trip's OWN rate overrides (None = inherited from driver/terminal), NOT
    /// effective rates. Lets the edit form prefill current override values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded_rate_per_mile: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadhead_rate_per_mile: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_stop_fee: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detention_rate_per_hour: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_dwell_minutes: Option<u32>,
    /// Stop facilities that are not yet geocoded — their missing coordinates are
    /// why `mileage_summary`/`total_miles` may be null. Populated on trip detail
    /// (create_trip / get_trip) so dispatchers see the blocker instead of a
    /// silently null mileage. Empty on list views.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub geocode_warnings: Vec<String>,
}

#[derive(serde::Serialize)]
pub struct FleetTripListResponse {
    pub items: Vec<FleetTripListItem>,
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
) -> FleetTripListItem {
    let driver_name = trip.driver_id.and_then(|did| driver_map.get(&did).cloned());
    let truck_unit  = trip.truck_id.and_then(|tid| truck_map.get(&tid).cloned());
    let trailer_units = trip.trailer_ids.iter()
        .filter_map(|tid| trailer_map.get(tid).cloned())
        .collect();

    let mut stops = trip.stops;
    for s in &mut stops { s.fill_utc_fields(); }

    FleetTripListItem {
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
        notes: trip.notes,
        deadhead_miles: trip.deadhead_miles,
        loaded_miles: trip.loaded_miles,
        total_miles: trip.total_miles,
        origin_facility_name: None,
        mileage_summary: None,
        // list shows frozen pay only; live pay on detail
        driver_pay: None,
        // raw overrides surfaced on detail only (set in build_trip_detail)
        loaded_rate_per_mile: None,
        deadhead_rate_per_mile: None,
        extra_stop_fee: None,
        detention_rate_per_hour: None,
        free_dwell_minutes: None,
        geocode_warnings: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Loads
// ---------------------------------------------------------------------------

#[utoipa::path(
    get,
    path = "/fleet/api/v1/loads",
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
    tag = "fleet"
)]
pub async fn list_loads(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Query(q): Query<ListLoadsQuery>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("loads:read")?;
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
    path = "/fleet/api/v1/loads/{id}",
    params(("id" = Uuid, Path, description = "Load UUID")),
    responses(
        (status = 200, description = "Load detail", body = LoadDetailResponse),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn get_load(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("loads:read")?;
    let record = state.db.get_load_by_id(id).await?;
    let response = build_load_detail(&state, record).await?;
    Ok(Json(response))
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/loads",
    request_body(content = CreateLoadRequest, description = "Load to create"),
    responses(
        (status = 201, description = "Created load", body = LoadDetailResponse),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn create_load(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Json(body): Json<CreateLoadRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("loads:write")?;
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
    path = "/fleet/api/v1/loads/{id}",
    params(("id" = Uuid, Path, description = "Load UUID")),
    request_body(content = UpdateLoadRequest, description = "Fields to update"),
    responses(
        (status = 200, description = "Updated load", body = LoadDetailResponse),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn update_load(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateLoadRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("loads:write")?;
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

    if let Some(ln) = body.load_number {
        updated = state.db.update_load_number(id, ln).await?;
    }

    let response = build_load_detail(&state, updated).await?;
    Ok(Json(response))
}

#[utoipa::path(
    delete,
    path = "/fleet/api/v1/loads/{id}",
    params(("id" = Uuid, Path, description = "Load UUID")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Load has active trips — cancel or complete them first"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn delete_load_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("loads:delete")?;
    state.db.get_load_by_id(id).await?;
    let active = state.db.count_active_trips_for_load(id).await?;
    if active > 0 {
        return Err(AppError::Conflict(format!(
            "load has {active} active trip(s); cancel or complete them first"
        )));
    }
    state.db.delete_load_by_id(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/loads/{id}/invoice",
    params(("id" = Uuid, Path, description = "Load UUID")),
    request_body(content = InvoiceActionRequest, description = "Optional invoice number and date"),
    responses(
        (status = 200, description = "Load transitioned to invoiced", body = LoadDetailResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Invalid status transition"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn invoice_load_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
    Json(body): Json<crate::models::InvoiceActionRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("loads:invoice")?;
    let record = state.db.transition_load_status(
        id, LoadStatus::Invoiced,
        body.invoice_number, body.invoice_date, None,
    ).await?;
    let response = build_load_detail(&state, record).await?;
    Ok(Json(response))
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/loads/{id}/cancel",
    params(("id" = Uuid, Path, description = "Load UUID")),
    request_body(content = CancelActionRequest, description = "Optional cancellation reason"),
    responses(
        (status = 200, description = "Load transitioned to cancelled", body = LoadDetailResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Invalid status transition"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn cancel_load_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
    Json(body): Json<crate::models::CancelActionRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("loads:write")?;
    let record = state.db.transition_load_status(
        id, LoadStatus::Cancelled, None, None, body.reason,
    ).await?;
    let response = build_load_detail(&state, record).await?;
    Ok(Json(response))
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/loads/{id}/settle",
    params(("id" = Uuid, Path, description = "Load UUID")),
    responses(
        (status = 200, description = "Load transitioned to settled", body = LoadDetailResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Invalid status transition"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn settle_load_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("loads:settle")?;
    let record = state.db.transition_load_status(
        id, LoadStatus::Settled, None, None, None,
    ).await?;
    let response = build_load_detail(&state, record).await?;
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
    pub trip_number: Option<String>,
    pub load_number: Option<String>,
    pub pay_period_start: Option<String>,
    pub pay_period_end: Option<String>,
}

#[utoipa::path(
    get,
    path = "/fleet/api/v1/trips",
    params(
        ("load_id" = Option<Uuid>, Query, description = "Filter by load ID"),
        ("driver_id" = Option<Uuid>, Query, description = "Filter by driver ID"),
        ("status" = Option<String>, Query, description = "Filter by status"),
        ("pay_period_start" = Option<String>, Query, description = "Only trips with pay_period_start >= this ISO date"),
        ("pay_period_end" = Option<String>, Query, description = "Only trips with pay_period_end <= this ISO date"),
    ),
    responses(
        (status = 200, description = "List of trips (enriched with driver/truck names)"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn list_trips(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Query(q): Query<ListTripsQuery>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trips:read")?;
    let items = build_trip_list_items(&state, q).await?;
    Ok(Json(FleetTripListResponse { items }))
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/trips",
    request_body(content = CreateTripRequest, description = "Trip to create"),
    responses(
        (status = 201, description = "Created trip (enriched with driver/truck names and mileage_summary)", body = FleetTripListItem),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Referenced load/driver/truck/trailer not found"),
        (status = 409, description = "Conflict — invalid assignment"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn create_trip_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Json(body): Json<crate::models::trip::CreateTripRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trips:write")?;
    // Create via the shared writer, which returns the created record — no
    // re-fetch (that races under concurrent creates).
    let record = crate::api::trips::apply_trip_create(&state, body).await?;
    let detail = build_trip_detail(&state, record.id).await?;
    Ok((StatusCode::CREATED, Json(detail)))
}

/// Shared list-trips builder used by the HTTP handler and the MCP `list_trips` tool.
/// Applies `trip_number` / `load_number` filters (post-query for trip_number, two-step
/// lookup for load_number), enriches with driver/truck/trailer names, and projects a
/// flat `origin_facility_name` from the previous trip's last stop facility.
pub async fn build_trip_list_items(
    state: &AppState,
    q: ListTripsQuery,
) -> Result<Vec<FleetTripListItem>, AppError> {
    // Resolve `load_number` → `load_id` if provided; if no such load exists, return [].
    let load_id_filter = if let Some(ln) = &q.load_number {
        match state.db.get_load_by_number(ln).await {
            Ok(load) => Some(load.id),
            Err(AppError::NotFound) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        }
    } else {
        q.load_id
    };

    let trips = state.db.list_trips(
        load_id_filter, q.driver_id, q.status.as_deref(),
        q.pay_period_start.as_deref(), q.pay_period_end.as_deref(),
    ).await?;

    // Apply `trip_number` filter post-fetch (case-sensitive exact match).
    let trips: Vec<_> = if let Some(tn) = &q.trip_number {
        trips.into_iter().filter(|t| &t.trip_number == tn).collect()
    } else {
        trips
    };

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

    // Collect previous_trip_id set for origin lookup, then batch the facility resolves.
    let prev_trip_ids: Vec<Uuid> = trips.iter()
        .filter_map(|t| t.previous_trip_id)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let mut origin_name_by_trip: std::collections::HashMap<Uuid, String> =
        std::collections::HashMap::new();
    if !prev_trip_ids.is_empty() {
        // Resolve each previous trip → last stop facility name.
        let mut fac_ids: Vec<Uuid> = Vec::new();
        let mut prev_to_fac: std::collections::HashMap<Uuid, Uuid> =
            std::collections::HashMap::new();
        for prev_id in &prev_trip_ids {
            if let Ok(prev) = state.db.get_trip(*prev_id).await {
                if let Some(fac_id) = prev.stops.last().and_then(|s| s.facility_id) {
                    fac_ids.push(fac_id);
                    prev_to_fac.insert(*prev_id, fac_id);
                }
            }
        }
        let facilities = state.db.batch_get_facilities(&fac_ids).await.unwrap_or_default();
        for trip in &trips {
            if let Some(prev_id) = trip.previous_trip_id {
                if let Some(fac_id) = prev_to_fac.get(&prev_id) {
                    if let Some(fac) = facilities.get(fac_id) {
                        origin_name_by_trip.insert(trip.id, fac.name.clone());
                    }
                }
            }
        }
    }

    let items: Vec<FleetTripListItem> = trips.into_iter()
        .map(|trip| {
            let trip_id = trip.id;
            let mut item = enrich_trip(trip, &driver_map, &truck_map, &trailer_map);
            item.origin_facility_name = origin_name_by_trip.get(&trip_id).cloned();
            item
        })
        .collect();
    Ok(items)
}

#[utoipa::path(
    get,
    path = "/fleet/api/v1/trips/{id}",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    responses(
        (status = 200, description = "Trip record (enriched with driver/truck names)"),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn get_trip(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trips:read")?;
    let item = build_trip_detail(&state, id).await?;
    Ok(Json(item))
}

/// Shared trip-detail builder — used by the HTTP handler and the MCP `get_trip` tool.
/// Returns the enriched `FleetTripListItem` carrying a full `mileage_summary`
/// (origin + legs[]) so callers get a single, coherent shape.
pub async fn build_trip_detail(
    state: &AppState,
    id: Uuid,
) -> Result<FleetTripListItem, AppError> {
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
    let summary = crate::api::mileage_summary::build_mileage_summary(state, &record).await;
    enriched.origin_facility_name = summary.origin.as_ref()
        .and_then(|o| o.facility_name.clone());
    enriched.mileage_summary = Some(summary);
    enriched.driver_pay = driver_pay_for_record(state, &record).await;
    // Surface the trip's OWN rate overrides so the edit form can prefill them.
    enriched.loaded_rate_per_mile = record.loaded_rate_per_mile;
    enriched.deadhead_rate_per_mile = record.deadhead_rate_per_mile;
    enriched.extra_stop_fee = record.extra_stop_fee;
    enriched.detention_rate_per_hour = record.detention_rate_per_hour;
    enriched.free_dwell_minutes = record.free_dwell_minutes;

    // Surface any stop facility that isn't geocoded — missing coordinates are why
    // trip mileage may come back null. Without this the failure is invisible to a
    // dispatcher until they notice the blank mileage. The repair path is
    // update_facility with explicit lat/lng (US Census can't resolve many new
    // industrial/warehouse addresses).
    let stop_fac_ids: Vec<Uuid> = record.stops.iter().filter_map(|s| s.facility_id).collect();
    if !stop_fac_ids.is_empty() {
        let facs = match state.db.batch_get_facilities(&stop_fac_ids).await {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(trip_id = %record.id, error = %e, "geocode-warning: batch_get_facilities failed");
                Default::default()
            }
        };
        let mut seen = std::collections::HashSet::new();
        for s in &record.stops {
            let Some(fid) = s.facility_id else { continue };
            if !seen.insert(fid) { continue } // one warning per facility (same dock can be pickup+delivery)
            let Some(f) = facs.get(&fid) else { continue };
            if f.geocode_status != crate::models::facility::GeocodeStatus::Ready {
                enriched.geocode_warnings.push(format!(
                    "Stop '{}' ({}) is not geocoded (status: {}); trip mileage cannot be computed until coordinates are set — fix via update_facility with lat/lng.",
                    f.name, f.address, f.geocode_status.as_str()
                ));
            }
        }
    }

    Ok(enriched)
}

/// Computes driver pay for a trip on read. Resolves rates (trip ?? driver ?? terminal
/// floor) and computes live. None when there are no loaded miles to pay on.
pub async fn driver_pay_for_record(
    state: &AppState,
    record: &crate::models::TripRecord,
) -> Option<crate::models::pay::DriverPay> {
    use crate::models::pay::*;
    // Frozen snapshot wins: a settled trip returns the pay captured at settlement.
    if let Some(snap) = &record.driver_pay_snapshot {
        return Some(snap.clone());
    }
    record.loaded_miles?; // no loaded miles -> no pay
    // Driver overrides + terminal floor.
    let driver = match record.driver_id {
        Some(did) => state.db.get_driver_by_id(did).await.ok(),
        None => None,
    };
    let terminal = {
        let tid = driver.as_ref().and_then(|d| d.terminal_id);
        let by_id = match tid { Some(t) => state.db.get_terminal_by_id(t).await.ok(), None => None };
        match by_id { Some(t) => t, None => state.db.default_terminal().await.ok()? }
    };
    let driver_ov = driver.as_ref().map(|d| RateOverrides {
        loaded_rate_per_mile: d.loaded_rate_per_mile,
        deadhead_rate_per_mile: d.deadhead_rate_per_mile,
        extra_stop_fee: d.extra_stop_fee,
        detention_rate_per_hour: d.detention_rate_per_hour,
        free_dwell_minutes: d.free_dwell_minutes,
    }).unwrap_or_default();
    let trip_ov = RateOverrides {
        loaded_rate_per_mile: record.loaded_rate_per_mile,
        deadhead_rate_per_mile: record.deadhead_rate_per_mile,
        extra_stop_fee: record.extra_stop_fee,
        detention_rate_per_hour: record.detention_rate_per_hour,
        free_dwell_minutes: record.free_dwell_minutes,
    };
    let floor = TerminalRates {
        loaded_rate_per_mile: terminal.loaded_rate_per_mile,
        deadhead_rate_per_mile: terminal.deadhead_rate_per_mile,
        extra_stop_fee: terminal.extra_stop_fee,
        detention_rate_per_hour: terminal.detention_rate_per_hour,
        free_dwell_minutes: terminal.free_dwell_minutes,
    };
    let rates = resolve_rates(&trip_ov, &driver_ov, &floor);
    let stops: Vec<PayStopInput> = record.stops.iter().map(|s| {
        let mut s2 = s.clone();
        s2.fill_utc_fields();
        PayStopInput {
            detention_free_minutes: s2.detention_free_minutes,
            actual_arrive_utc: s2.actual_arrive_utc,
            actual_depart_utc: s2.actual_depart_utc,
        }
    }).collect();
    Some(compute_driver_pay(record.loaded_miles, record.deadhead_miles, &stops, &rates))
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/trips/{id}/assign",
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
    tag = "fleet"
)]
pub async fn assign_trip(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
    Json(body): Json<AssignTripRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trips:write")?;
    // Thin wrapper over the shared lifecycle so REST and MCP behave identically
    // (eligibility rules live in trip_lifecycle::validate_assignment).
    let trip = crate::services::trip_lifecycle::assign(&state, id, body).await?;
    Ok(Json(trip))
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/trips/{id}/unassign",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    responses(
        (status = 200, description = "Trip unassigned", body = TripRecord),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — invalid status transition"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn unassign_trip(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trips:write")?;
    // Thin wrapper over the shared lifecycle (see assign_trip): one implementation
    // for REST and MCP, including the "don't demote a still-dispatched resource" guard.
    let trip = crate::services::trip_lifecycle::unassign(&state, id).await?;
    Ok(Json(trip))
}

// ---------------------------------------------------------------------------
// Trip lifecycle actions — thin wrappers around `services::trip_lifecycle`.
// Same business logic; separate utoipa annotations to surface under the
// fleet_user path/tag.
// ---------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/fleet/api/v1/trips/{id}/dispatch",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    responses(
        (status = 200, description = "Trip dispatched", body = TripRecord),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — trip must be in assigned status"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn dispatch_trip(
    state: State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    id: Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trips:write")?;
    let trip = crate::services::trip_lifecycle::dispatch(&state, id.0).await?;
    Ok(Json(trip))
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/trips/{id}/undispatch",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    responses(
        (status = 200, description = "Trip undispatched back to assigned", body = TripRecord),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — trip must be in dispatched status (not in_transit or beyond)"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn undispatch_trip(
    state: State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    id: Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trips:write")?;
    let trip = crate::services::trip_lifecycle::undispatch(&state, id.0).await?;
    Ok(Json(trip))
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/trips/{id}/cancel",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    responses(
        (status = 200, description = "Trip cancelled", body = TripRecord),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — cannot cancel a trip that is in_transit or delivered"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn cancel_trip(
    state: State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    id: Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trips:write")?;
    let trip = crate::services::trip_lifecycle::cancel(&state, id.0).await?;
    Ok(Json(trip))
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/trips/{id}/complete",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    responses(
        (status = 204, description = "Trip completed and resources released"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — trip must be in delivered status"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn complete_trip(
    state: State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    id: Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trips:write")?;
    crate::services::trip_lifecycle::complete(&state, id.0).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/trips/{id}/stops/{seq}/arrive",
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
    tag = "fleet"
)]
pub async fn stop_arrive(
    state: State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    path: Path<(Uuid, u32)>,
    body: Json<StopArriveRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trips:write")?;
    let (id, seq) = path.0;
    // Edit-lock: a settled trip's stop times are frozen.
    let existing = state.db.get_trip(id).await?;
    if existing.settlement_ref.is_some() {
        return Err(AppError::Conflict("trip is settled; stop times are frozen".into()));
    }
    let stop_tz = existing.stops.iter()
        .find(|s| s.sequence == seq)
        .ok_or(AppError::NotFound)?
        .timezone
        .clone();
    crate::services::trip_stops::validate_arrive(&body.actual_arrive, stop_tz.as_deref())?;
    let mut trip = crate::services::trip_stops::record_stop_arrive(
        &state, id, seq, body.0.actual_arrive,
    ).await?;
    for s in &mut trip.stops { s.fill_utc_fields(); }
    Ok(Json(trip))
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/trips/{id}/stops/{seq}/depart",
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
    tag = "fleet"
)]
pub async fn stop_depart(
    state: State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    path: Path<(Uuid, u32)>,
    body: Json<StopDepartRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trips:write")?;
    let (id, seq) = path.0;
    // Edit-lock: a settled trip's stop times are frozen.
    let existing = state.db.get_trip(id).await?;
    if existing.settlement_ref.is_some() {
        return Err(AppError::Conflict("trip is settled; stop times are frozen".into()));
    }
    let stop_tz = existing.stops.iter()
        .find(|s| s.sequence == seq)
        .ok_or(AppError::NotFound)?
        .timezone
        .clone();
    crate::services::trip_stops::validate_depart(&body.actual_depart, stop_tz.as_deref())?;
    let mut trip = crate::services::trip_stops::record_stop_depart(
        &state, id, seq, body.0.actual_depart,
    ).await?;
    for s in &mut trip.stops { s.fill_utc_fields(); }
    Ok(Json(trip))
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/trips/{id}/stops/{seq}/late",
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
    tag = "fleet"
)]
pub async fn stop_late(
    state: State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    path: Path<(Uuid, u32)>,
    body: Json<StopLateRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trips:write")?;
    let (id, seq) = path.0;
    crate::services::trip_lifecycle::stop_late(&state, id, seq, body.0).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/trips/{id}/check-call",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    request_body(content = CheckCallRequest, description = "Location, notes, and optional next-stop ETA"),
    responses(
        (status = 204, description = "Check call recorded"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn check_call(
    state: State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    id: Path<Uuid>,
    body: Json<CheckCallRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trips:write")?;
    crate::services::trip_lifecycle::check_call(&state, id.0, body.0).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
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
    path = "/fleet/api/v1/drivers",
    params(
        ("status" = Option<String>, Query, description = "Filter by status"),
    ),
    responses(
        (status = 200, description = "List of drivers", body = DriverListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn list_drivers(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Query(q): Query<ListDriversQuery>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("drivers:read")?;
    let (total, items) = state.db.list_drivers(q.status.as_deref(), 100, 0).await?;
    Ok(Json(DriverListResponse { returned: total, items }))
}

#[utoipa::path(
    get,
    path = "/fleet/api/v1/drivers/{id}",
    params(("id" = Uuid, Path, description = "Driver UUID")),
    responses(
        (status = 200, description = "Driver record", body = DriverRecord),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn get_driver(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("drivers:read")?;
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
    path = "/fleet/api/v1/trucks",
    params(
        ("status" = Option<String>, Query, description = "Filter by status"),
    ),
    responses(
        (status = 200, description = "List of trucks", body = TruckListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn list_trucks(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Query(q): Query<ListTrucksQuery>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trucks:read")?;
    let (total, items) = state.db.list_trucks(q.status.as_deref(), 100, 0).await?;
    Ok(Json(TruckListResponse { returned: total, items }))
}

#[utoipa::path(
    get,
    path = "/fleet/api/v1/trucks/{id}",
    params(("id" = Uuid, Path, description = "Truck UUID")),
    responses(
        (status = 200, description = "Truck record", body = TruckRecord),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn get_truck(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trucks:read")?;
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
    path = "/fleet/api/v1/trailers",
    params(
        ("status" = Option<String>, Query, description = "Filter by status"),
    ),
    responses(
        (status = 200, description = "List of trailers", body = TrailerListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn list_trailers(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Query(q): Query<ListTrailersQuery>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trailers:read")?;
    let (total, items) = state.db.list_trailers(q.status.as_deref(), None, 100, 0).await?;
    Ok(Json(TrailerListResponse { returned: total, items }))
}

#[utoipa::path(
    get,
    path = "/fleet/api/v1/trailers/{id}",
    params(("id" = Uuid, Path, description = "Trailer UUID")),
    responses(
        (status = 200, description = "Trailer record", body = TrailerRecord),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn get_trailer(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trailers:read")?;
    let record = state.db.get_trailer_by_id(id).await?;
    Ok(Json(record))
}

// ---------------------------------------------------------------------------
// Maintenance
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListMaintenanceQuery {
    pub equipment_type: Option<String>,
    pub equipment_id: Option<uuid::Uuid>,
    pub category: Option<String>,
}

#[utoipa::path(
    get,
    path = "/fleet/api/v1/maintenance",
    params(
        ("equipment_type" = Option<String>, Query, description = "Filter by equipment type (truck/trailer)"),
        ("equipment_id" = Option<Uuid>, Query, description = "Filter by equipment UUID"),
        ("category" = Option<String>, Query, description = "Filter by category"),
    ),
    responses(
        (status = 200, description = "List of maintenance entries", body = MaintenanceListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn list_maintenance(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Query(q): Query<ListMaintenanceQuery>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("maintenance:read")?;
    let equipment_id = q.equipment_id.map(|id| id.to_string());
    let (total, items) = state.db.list_maintenance(
        q.equipment_type.as_deref(),
        equipment_id.as_deref(),
        q.category.as_deref(),
        100,
        0,
    ).await?;
    Ok(Json(MaintenanceListResponse { returned: total, items }))
}

#[utoipa::path(
    get,
    path = "/fleet/api/v1/maintenance/{id}",
    params(("id" = Uuid, Path, description = "Maintenance UUID")),
    responses(
        (status = 200, description = "Maintenance record", body = MaintenanceRecord),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn get_maintenance(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("maintenance:read")?;
    let record = state.db.get_maintenance_by_id(id).await?;
    Ok(Json(record))
}

// ---------------------------------------------------------------------------
// Facilities
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListFacilitiesDispatchQuery {
    /// Substring search across name and address (case-insensitive).
    pub q: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub include_archived: Option<bool>,
}

#[utoipa::path(
    get,
    path = "/fleet/api/v1/facilities",
    params(
        ("q" = Option<String>, Query, description = "Substring search across name and address (case-insensitive)"),
        ("limit" = Option<usize>, Query, description = "Max results (default and max 1000, the scan cap)"),
        ("offset" = Option<usize>, Query, description = "Pagination offset"),
        ("include_archived" = Option<bool>, Query, description = "When true, include archived facilities (default false)"),
    ),
    responses(
        (status = 200, description = "List of facilities", body = crate::models::FacilityListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn list_facilities(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Query(q): Query<ListFacilitiesDispatchQuery>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("facilities:read")?;
    // Follow the fleet list "scan cap" pattern: fetch up to the cap, apply the
    // `q` filter in memory, and return everything (the UI sends no limit and
    // renders all rows). Callers may still pass an explicit limit/offset to
    // page. At current facility volume (~tens) the in-memory filter is cheap;
    // if the table grows large enough to matter, push the OR-LIKE filter into
    // `build_facility_filter` so LanceDB does the work.
    const SCAN_CAP: usize = 1000;
    let limit = q.limit.unwrap_or(SCAN_CAP).min(SCAN_CAP);
    let offset = q.offset.unwrap_or(0);
    let include_archived = q.include_archived.unwrap_or(false);
    let (_total, items) = state.db.list_facilities(None, &[], SCAN_CAP, 0, include_archived).await?;

    let filtered: Vec<_> = if let Some(query) = q.q.as_deref().filter(|s| !s.is_empty()) {
        let needle = query.to_lowercase();
        items.into_iter()
            .filter(|f| {
                f.name.to_lowercase().contains(&needle)
                    || f.address.to_lowercase().contains(&needle)
            })
            .collect()
    } else {
        items
    };

    let page: Vec<_> = filtered.into_iter().skip(offset).take(limit).collect();
    let returned = page.len();
    Ok(Json(crate::models::FacilityListResponse { returned, items: page }))
}

#[utoipa::path(
    get,
    path = "/fleet/api/v1/facilities/{id}",
    params(("id" = Uuid, Path, description = "Facility UUID")),
    responses(
        (status = 200, description = "Facility record", body = crate::models::FacilityRecord),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Facility not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn get_facility(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("facilities:read")?;
    let record = state.db.get_facility_by_id(id).await?;
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

fn trip_subject(t: &crate::models::TripRecord) -> String {
    let route = if t.stops.len() >= 2 {
        let o = t.stops.first().and_then(|s| s.name.as_deref()).unwrap_or("?");
        let d = t.stops.last().and_then(|s| s.name.as_deref()).unwrap_or("?");
        format!(" · {o} → {d}")
    } else {
        String::new()
    };
    format!("Trip {}{}", t.trip_number, route)
}

async fn subject_for(db: &crate::db::DbClient, entity_type: &str, id: Uuid) -> Option<String> {
    match entity_type {
        "trip" => db.get_trip(id).await.ok().map(|t| trip_subject(&t)),
        "driver" => db.get_driver_by_id(id).await.ok().map(|d| d.name),
        "truck" => db.get_truck_by_id(id).await.ok().map(|t| format!("Truck {}", t.unit_number)),
        "trailer" => db.get_trailer_by_id(id).await.ok().map(|t| format!("Trailer {}", t.unit_number)),
        "blob" => db.get_by_id(id).await.ok().map(|b| b.name),
        _ => None,
    }
}

#[utoipa::path(
    get,
    path = "/fleet/api/v1/events",
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
    tag = "fleet"
)]
pub async fn list_events(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Query(q): Query<ListEventsDispatchQuery>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("events:read")?;
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
    let mut items: Vec<EventResponse> = records.into_iter().map(EventResponse::from).collect();

    // Resolve subject labels, deduping lookups by (entity_type, entity_id).
    let mut cache: std::collections::HashMap<(String, Uuid), Option<String>> =
        std::collections::HashMap::new();
    for it in items.iter_mut() {
        let key = (it.entity_type.clone(), it.entity_id);
        let label = match cache.get(&key) {
            Some(v) => v.clone(),
            None => {
                let v = subject_for(&state.db, &it.entity_type, it.entity_id).await;
                cache.insert(key, v.clone());
                v
            }
        };
        it.subject = label;
    }

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
    get, path = "/fleet/api/v1/loads/count",
    responses(
        (status = 200, description = "Open load count"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn count_open_loads(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("loads:read")?;
    let filter = Some("status = 'planned' OR status = 'assigned' OR status = 'dispatched' OR status = 'in_transit'".to_string());
    let count = state.db.load_table.count_rows(filter).await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(CountResponse { count }))
}

/// Count drivers with active status.
#[utoipa::path(
    get, path = "/fleet/api/v1/drivers/count",
    responses(
        (status = 200, description = "Active driver count"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn count_active_drivers(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("drivers:read")?;
    let filter = Some("status = 'available' OR status = 'assigned' OR status = 'dispatched'".to_string());
    let count = state.db.driver_table.count_rows(filter).await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(CountResponse { count }))
}

/// Count blobs with pending status.
#[utoipa::path(
    get, path = "/fleet/api/v1/blobs/count",
    responses(
        (status = 200, description = "Pending document count"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn count_pending_documents(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("blobs:read")?;
    let filter = Some("status = 'pending'".to_string());
    let count = state.db.blob_table.count_rows(filter).await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(CountResponse { count }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::trip::{TripRecord, TripStatus, TripStop, TripStopType};
    use crate::models::{DriverRecord, DriverStatus};
    use tempfile::TempDir;

    async fn test_db() -> (crate::db::DbClient, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = crate::db::DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        (db, dir)
    }

    fn make_stop(seq: u32, name: Option<&str>) -> TripStop {
        TripStop {
            sequence: seq,
            stop_type: TripStopType::Pickup,
            facility_id: None,
            name: name.map(|s| s.to_string()),
            address: None,
            load_stop_index: None,
            scheduled_arrive: None,
            scheduled_arrive_end: None,
            actual_arrive: None,
            actual_depart: None,
            expected_dwell_minutes: None,
            detention_free_minutes: None,
            detention_grace_minutes: None,
            notes: None,
            timezone: None,
            actual_arrive_utc: None,
            actual_depart_utc: None,
        }
    }

    fn make_trip(id: Uuid, trip_number: &str, stops: Vec<TripStop>) -> TripRecord {
        let now = chrono::Utc::now();
        TripRecord {
            id,
            trip_number: trip_number.into(),
            load_id: None,
            load_number: None,
            previous_trip_id: None,
            deadhead_miles: None,
            loaded_miles: None,
            total_miles: None,
            segment_miles: vec![],
            sequence: 0,
            driver_id: None,
            truck_id: None,
            trailer_ids: vec![],
            status: TripStatus::Planned,
            stops,
            notes: None,
            blob_ids: vec![],
            loaded_rate_per_mile: None,
            deadhead_rate_per_mile: None,
            extra_stop_fee: None,
            detention_rate_per_hour: None,
            free_dwell_minutes: None,
            settlement_ref: None,
            pay_period_start: None,
            pay_period_end: None,
            driver_pay_snapshot: None,
            embedding: None,
            owner_id: 0,
            created_at: now,
            updated_at: now,
        }
    }

    fn sample_driver(name: &str) -> DriverRecord {
        let now = chrono::Utc::now();
        DriverRecord {
            id: Uuid::new_v4(),
            name: name.into(),
            phone: None,
            email: None,
            license_number: None,
            license_state: None,
            license_expiry: None,
            status: DriverStatus::Available,
            notes: None,
            current_truck_id: None,
            current_trailer_ids: vec![],
            blob_ids: vec![],
            embedding: None,
            owner_id: 0,
            created_at: now,
            updated_at: now,
            terminal_id: None,
            loaded_rate_per_mile: None,
            deadhead_rate_per_mile: None,
            extra_stop_fee: None,
            detention_rate_per_hour: None,
            free_dwell_minutes: None,
        }
    }

    // --- trip_subject (pure, no DB) ---

    #[test]
    fn test_trip_subject_with_two_stops() {
        let trip = make_trip(Uuid::new_v4(), "T-2026-0042", vec![
            make_stop(0, Some("A")),
            make_stop(1, Some("B")),
        ]);
        assert_eq!(trip_subject(&trip), "Trip T-2026-0042 · A → B");
    }

    #[test]
    fn test_trip_subject_with_one_stop() {
        let trip = make_trip(Uuid::new_v4(), "T-2026-0042", vec![
            make_stop(0, Some("A")),
        ]);
        assert_eq!(trip_subject(&trip), "Trip T-2026-0042");
    }

    #[test]
    fn test_trip_subject_with_no_stops() {
        let trip = make_trip(Uuid::new_v4(), "T-2026-0042", vec![]);
        assert_eq!(trip_subject(&trip), "Trip T-2026-0042");
    }

    // --- subject_for (requires DB) ---

    #[tokio::test]
    async fn test_subject_for_driver_found() {
        let (db, _dir) = test_db().await;
        let driver = sample_driver("Jane Doe");
        db.insert_driver(&driver).await.unwrap();
        let result = subject_for(&db, "driver", driver.id).await;
        assert_eq!(result, Some("Jane Doe".into()));
    }

    #[tokio::test]
    async fn test_subject_for_driver_missing() {
        let (db, _dir) = test_db().await;
        let result = subject_for(&db, "driver", Uuid::new_v4()).await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_subject_for_unknown_entity_type() {
        let (db, _dir) = test_db().await;
        let result = subject_for(&db, "bogus_type", Uuid::new_v4()).await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_subject_for_trip() {
        let (db, _dir) = test_db().await;
        let trip = make_trip(Uuid::new_v4(), "T-99", vec![
            make_stop(0, Some("Origin")),
            make_stop(1, Some("Dest")),
        ]);
        db.insert_trip(&trip).await.unwrap();
        let result = subject_for(&db, "trip", trip.id).await;
        assert_eq!(result, Some("Trip T-99 · Origin → Dest".into()));
    }
}

/// Count events that occurred today (UTC).
#[utoipa::path(
    get, path = "/fleet/api/v1/events/count",
    responses(
        (status = 200, description = "Events today count"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn count_events_today(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("events:read")?;
    let today = chrono::Utc::now().date_naive().format("%Y-%m-%dT00:00:00Z").to_string();
    let filter = Some(format!("occurred_at >= '{today}'"));
    let count = state.db.event_table.count_rows(filter).await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(CountResponse { count }))
}

// ---------------------------------------------------------------------------
// Internal helpers — mirror build_detail_response from src/api/loads.rs
// ---------------------------------------------------------------------------

pub async fn build_load_detail(
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
        trips.sort_by_key(|t| std::cmp::Reverse(t.created_at));
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
