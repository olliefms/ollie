// src/api/loads.rs
use crate::{
    ai::embed::embed_text,
    api::facilities::resolve_or_create_facility,
    error::AppError,
    models::{
        CancelActionRequest, CreateLoadRequest, FacilityResolutionResponse,
        InvoiceActionRequest, LoadDetailResponse, LoadListResponse, LoadRecord,
        LoadStatus, Stop, StopInput, StopResponse, UpdateLoadRequest,
    },
    AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use axum_extra::extract::Query;
use chrono::Utc;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoadStopArriveRequest {
    pub actual_arrive: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoadStopDepartRequest {
    pub actual_depart: String,
}

#[derive(Deserialize, Default, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListLoadsQuery {
    /// Semantic search query — triggers vector search when present
    pub s: Option<String>,
    /// Filter by status (planned, dispatched, in_transit, delivered, invoiced, settled, cancelled)
    pub status: Option<String>,
    /// Filter by customer name (substring match)
    pub customer: Option<String>,
    /// Filter by created_at >= this date (ISO 8601, e.g. 2024-01-01)
    pub from: Option<String>,
    /// Filter by created_at <= this date (ISO 8601, e.g. 2024-12-31)
    pub to: Option<String>,
    /// Filter by tag (repeat for multiple: ?tag=a&tag=b)
    #[serde(default)]
    pub tag: Vec<String>,
    /// Maximum results (default 20, max 100)
    pub limit: Option<usize>,
    /// Pagination offset (default 0)
    pub offset: Option<usize>,
}

#[utoipa::path(
    post,
    path = "/api/v1/loads",
    request_body(content = CreateLoadRequest, description = "Load to create"),
    responses(
        (status = 201, description = "Created load detail", body = LoadDetailResponse),
        (status = 200, description = "Facility resolution required — ambiguous stop facility", body = Vec<FacilityResolutionResponse>),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "loads"
)]
pub async fn create_load(
    State(state): State<AppState>,
    Json(body): Json<CreateLoadRequest>,
) -> Result<impl IntoResponse, AppError> {
    let stops = resolve_stops(&state, body.stops).await?;
    let now = Utc::now();

    let load_number = match body.load_number {
        Some(n) => n,
        None => { use chrono::Datelike; state.db.next_load_number(now.year()).await? },
    };

    let facility_ids: Vec<Uuid> = stops.iter().map(|s| s.facility_id).collect();
    let facilities = state.db.batch_get_facilities(&facility_ids).await?;
    let stop_text = stops.iter()
        .filter_map(|s| facilities.get(&s.facility_id))
        .map(|f| format!("{} {}", f.name, f.address))
        .collect::<Vec<_>>().join(" ");
    let embed_text_str = format!(
        "{} {} {} {} {}",
        body.customer_name, stop_text,
        body.commodity.as_deref().unwrap_or(""),
        body.notes.as_deref().unwrap_or(""),
        body.tags.join(" "),
    );
    let embedding = embed_text(&state.ai, &embed_text_str).await.ok();

    let record = LoadRecord {
        id: Uuid::new_v4(), load_number, owner_id: 0,
        status: LoadStatus::Planned,
        customer_name: body.customer_name, customer_ref: body.customer_ref,
        stops, rate_items: body.rate_items,
        commodity: body.commodity, weight_lbs: body.weight_lbs,
        miles: body.miles, notes: body.notes, tags: body.tags,
        blob_ids: body.blob_ids, invoice_number: None, invoice_date: None,
        cancellation_reason: None, embedding, created_at: now, updated_at: now,
    };

    state.db.insert_load(&record).await?;

    if record.miles.is_none() {
        let _ = state.routing_tx.try_send(record.id);
    }

    let response = build_detail_response(&state, record).await?;
    Ok((StatusCode::CREATED, Json(response)))
}

#[utoipa::path(
    get,
    path = "/api/v1/loads",
    params(ListLoadsQuery),
    responses(
        (status = 200, description = "List of loads (or semantic search results when ?s= is provided)", body = LoadListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "loads"
)]
pub async fn list_loads(
    State(state): State<AppState>,
    Query(q): Query<ListLoadsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let limit = q.limit.unwrap_or(20).min(100);
    let offset = q.offset.unwrap_or(0);

    if let Some(query_text) = q.s {
        let embedding = embed_text(&state.ai, &query_text).await?;
        let items = state.db.search_loads(
            embedding, q.status.as_deref(), q.customer.as_deref(), &q.tag, limit,
        ).await?;
        let returned = items.len();
        return Ok(Json(LoadListResponse { returned, items }));
    }

    let (total, items) = state.db.list_loads(
        q.status.as_deref(), q.customer.as_deref(), &q.tag,
        q.from.as_deref(), q.to.as_deref(), limit, offset,
    ).await?;
    Ok(Json(LoadListResponse { returned: total, items }))
}

#[utoipa::path(
    get,
    path = "/api/v1/loads/{id}",
    params(
        ("id" = Uuid, Path, description = "Load UUID")
    ),
    responses(
        (status = 200, description = "Load detail with expanded stop information", body = LoadDetailResponse),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "loads"
)]
pub async fn get_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_load_by_id(id).await?;
    let response = build_detail_response(&state, record).await?;
    Ok(Json(response))
}

#[utoipa::path(
    patch,
    path = "/api/v1/loads/{id}",
    params(
        ("id" = Uuid, Path, description = "Load UUID")
    ),
    request_body(content = UpdateLoadRequest, description = "Fields to update — all optional. Unknown fields in the request body are silently ignored."),
    responses(
        (status = 200, description = "Updated load detail", body = LoadDetailResponse),
        (status = 200, description = "Facility resolution required — ambiguous stop facility", body = Vec<FacilityResolutionResponse>),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "loads"
)]
pub async fn update_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateLoadRequest>,
) -> Result<impl IntoResponse, AppError> {
    let stops_provided = body.stops.is_some();
    let stops = match body.stops {
        Some(inputs) => Some(resolve_stops(&state, inputs).await?),
        None => None,
    };

    let existing = state.db.get_load_by_id(id).await?;
    let effective_stops = stops.as_ref().unwrap_or(&existing.stops);
    let facility_ids: Vec<Uuid> = effective_stops.iter().map(|s| s.facility_id).collect();
    let facilities = state.db.batch_get_facilities(&facility_ids).await?;
    let stop_text = effective_stops.iter()
        .filter_map(|s| facilities.get(&s.facility_id))
        .map(|f| format!("{} {}", f.name, f.address))
        .collect::<Vec<_>>().join(" ");
    let embed_text_str = format!(
        "{} {} {} {} {}",
        body.customer_name.as_deref().unwrap_or(&existing.customer_name),
        stop_text,
        body.commodity.as_deref().unwrap_or(existing.commodity.as_deref().unwrap_or("")),
        body.notes.as_deref().unwrap_or(existing.notes.as_deref().unwrap_or("")),
        body.tags.as_ref().unwrap_or(&existing.tags).join(" "),
    );
    let embedding = embed_text(&state.ai, &embed_text_str).await.ok();

    let mut updated = state.db.update_load_metadata(
        id, body.customer_name, body.customer_ref, stops,
        body.rate_items, body.commodity, body.weight_lbs, body.miles,
        body.notes, body.tags, body.blob_ids, embedding,
    ).await?;

    if stops_provided && body.miles.is_none() {
        state.db.clear_load_miles(id).await?;
        updated.miles = None;
        let _ = state.routing_tx.try_send(id);
    }

    let response = build_detail_response(&state, updated).await?;
    Ok(Json(response))
}

#[utoipa::path(
    delete,
    path = "/api/v1/loads/{id}",
    params(
        ("id" = Uuid, Path, description = "Load UUID")
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 409, description = "Load has active trips — cancel or complete them first"),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "loads"
)]
pub async fn delete_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
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
    path = "/api/v1/loads/{id}/invoice",
    params(("id" = Uuid, Path, description = "Load UUID")),
    request_body(content = InvoiceActionRequest, description = "Optional invoice number and date"),
    responses(
        (status = 200, description = "Load transitioned to invoiced", body = LoadDetailResponse),
        (status = 409, description = "Invalid status transition"),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "loads"
)]
pub async fn invoice_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<InvoiceActionRequest>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.transition_load_status(
        id, LoadStatus::Invoiced,
        body.invoice_number, body.invoice_date, None,
    ).await?;
    let response = build_detail_response(&state, record).await?;
    Ok(Json(response))
}

#[utoipa::path(
    post,
    path = "/api/v1/loads/{id}/cancel",
    params(("id" = Uuid, Path, description = "Load UUID")),
    request_body(content = CancelActionRequest, description = "Optional cancellation reason"),
    responses(
        (status = 200, description = "Load transitioned to cancelled", body = LoadDetailResponse),
        (status = 409, description = "Invalid status transition — cannot cancel delivered/invoiced/settled loads"),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "loads"
)]
pub async fn cancel_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<CancelActionRequest>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.transition_load_status(
        id, LoadStatus::Cancelled, None, None, body.reason,
    ).await?;
    let response = build_detail_response(&state, record).await?;
    Ok(Json(response))
}

#[utoipa::path(
    post,
    path = "/api/v1/loads/{id}/settle",
    params(("id" = Uuid, Path, description = "Load UUID")),
    responses(
        (status = 200, description = "Load transitioned to settled", body = LoadDetailResponse),
        (status = 409, description = "Invalid status transition"),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "loads"
)]
pub async fn settle_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.transition_load_status(
        id, LoadStatus::Settled, None, None, None,
    ).await?;
    let response = build_detail_response(&state, record).await?;
    Ok(Json(response))
}

#[utoipa::path(
    post,
    path = "/api/v1/loads/{id}/stops/{seq}/arrive",
    params(
        ("id" = uuid::Uuid, Path, description = "Load UUID"),
        ("seq" = u32, Path, description = "Stop sequence number"),
    ),
    request_body(content = LoadStopArriveRequest, description = "Actual arrival time"),
    responses(
        (status = 200, description = "Stop arrival recorded", body = LoadDetailResponse),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "loads"
)]
pub async fn load_stop_arrive(
    State(state): State<AppState>,
    Path((id, seq)): Path<(uuid::Uuid, u32)>,
    Json(body): Json<LoadStopArriveRequest>,
) -> Result<impl IntoResponse, AppError> {
    let load = state.db.get_load_by_id(id).await?;
    let stop_tz = load.stops.iter()
        .find(|s| s.sequence == seq)
        .ok_or(AppError::NotFound)?
        .timezone
        .clone();
    if let Some(tz_str) = stop_tz.as_deref() {
        crate::models::load::validate_stop_time_str(&body.actual_arrive, tz_str, "actual_arrive")?;
    }
    let mut updated_stops = load.stops.clone();
    for stop in &mut updated_stops {
        if stop.sequence == seq {
            stop.actual_arrive = Some(body.actual_arrive.clone());
            break;
        }
    }
    let updated = state.db.update_load_metadata(
        id, None, None, Some(updated_stops),
        None, None, None, None, None, None, None, None,
    ).await?;
    let response = build_detail_response(&state, updated).await?;
    Ok(Json(response))
}

#[utoipa::path(
    post,
    path = "/api/v1/loads/{id}/stops/{seq}/depart",
    params(
        ("id" = uuid::Uuid, Path, description = "Load UUID"),
        ("seq" = u32, Path, description = "Stop sequence number"),
    ),
    request_body(content = LoadStopDepartRequest, description = "Actual departure time"),
    responses(
        (status = 200, description = "Stop departure recorded", body = LoadDetailResponse),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "loads"
)]
pub async fn load_stop_depart(
    State(state): State<AppState>,
    Path((id, seq)): Path<(uuid::Uuid, u32)>,
    Json(body): Json<LoadStopDepartRequest>,
) -> Result<impl IntoResponse, AppError> {
    let load = state.db.get_load_by_id(id).await?;
    let stop_tz = load.stops.iter()
        .find(|s| s.sequence == seq)
        .ok_or(AppError::NotFound)?
        .timezone
        .clone();
    if let Some(tz_str) = stop_tz.as_deref() {
        crate::models::load::validate_stop_time_str(&body.actual_depart, tz_str, "actual_depart")?;
    }
    let mut updated_stops = load.stops.clone();
    for stop in &mut updated_stops {
        if stop.sequence == seq {
            stop.actual_depart = Some(body.actual_depart.clone());
            break;
        }
    }
    let updated = state.db.update_load_metadata(
        id, None, None, Some(updated_stops),
        None, None, None, None, None, None, None, None,
    ).await?;
    let response = build_detail_response(&state, updated).await?;
    Ok(Json(response))
}

async fn resolve_stops(state: &AppState, inputs: Vec<StopInput>) -> Result<Vec<Stop>, AppError> {
    let mut stops = Vec::new();
    let mut resolutions: Vec<FacilityResolutionResponse> = Vec::new();

    for (idx, input) in inputs.into_iter().enumerate() {
        if !input.service_type.is_valid_for(&input.stop_type) {
            return Err(AppError::BadRequest(format!(
                "service_type '{}' is not valid for stop_type '{}'",
                input.service_type.as_str(), input.stop_type.as_str()
            )));
        }

        let _: chrono_tz::Tz = input.timezone.parse().map_err(|_| {
            AppError::UnprocessableEntity(format!(
                "stop {}: '{}' is not a valid IANA timezone",
                input.sequence, input.timezone
            ))
        })?;

        crate::models::load::validate_stop_time_str(
            &input.scheduled_arrive, &input.timezone, "scheduled_arrive",
        )?;
        if let Some(ref end) = input.scheduled_arrive_end {
            crate::models::load::validate_stop_time_str(end, &input.timezone, "scheduled_arrive_end")?;
        }

        let facility_id = if let Some(id) = input.facility_id {
            state.db.get_facility_by_id(id).await?;
            id
        } else {
            let name = input.facility_name.ok_or_else(|| AppError::BadRequest(
                "stop must provide either facility_id or facility_name + address".into()
            ))?;
            let address = input.address.ok_or_else(|| AppError::BadRequest(
                "stop must provide address when facility_id is not given".into()
            ))?;
            match resolve_or_create_facility(state, &name, &address, input.force_new_facility).await {
                Ok(id) => id,
                Err(AppError::FacilityResolution(res)) => {
                    let mut inner = *res;
                    for r in &mut inner { r.stop_index = idx; }
                    resolutions.extend(inner);
                    continue;
                }
                Err(e) => return Err(e),
            }
        };

        stops.push(Stop {
            sequence: input.sequence,
            stop_type: input.stop_type,
            service_type: input.service_type,
            facility_id,
            scheduled_arrive: input.scheduled_arrive,
            scheduled_arrive_end: input.scheduled_arrive_end,
            actual_arrive: input.actual_arrive,
            actual_depart: input.actual_depart,
            expected_dwell_minutes: input.expected_dwell_minutes,
            detention_free_minutes: input.detention_free_minutes,
            detention_grace_minutes: input.detention_grace_minutes,
            notes: input.notes,
            blob_ids: input.blob_ids,
            timezone: Some(input.timezone),
        });
    }

    if !resolutions.is_empty() {
        return Err(AppError::FacilityResolution(Box::new(resolutions)));
    }

    Ok(stops)
}

async fn build_detail_response(
    state: &AppState,
    record: LoadRecord,
) -> Result<LoadDetailResponse, AppError> {
    let facility_ids: Vec<Uuid> = record.stops.iter().map(|s| s.facility_id).collect();
    let facilities = state.db.batch_get_facilities(&facility_ids).await?;

    let stops: Vec<StopResponse> = record.stops.iter().map(|stop| {
        let facility = facilities.get(&stop.facility_id);
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
        }
    }).collect();

    let total_rate_usd = record.total_rate_usd();
    Ok(LoadDetailResponse {
        id: record.id, load_number: record.load_number, status: record.status,
        customer_name: record.customer_name, customer_ref: record.customer_ref,
        stops, rate_items: record.rate_items, total_rate_usd,
        commodity: record.commodity, weight_lbs: record.weight_lbs, miles: record.miles,
        notes: record.notes, tags: record.tags, blob_ids: record.blob_ids,
        invoice_number: record.invoice_number, invoice_date: record.invoice_date,
        cancellation_reason: record.cancellation_reason,
        created_at: record.created_at, updated_at: record.updated_at,
    })
}
