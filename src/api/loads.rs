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
use uuid::Uuid;

#[derive(Deserialize, Default)]
pub struct ListLoadsQuery {
    pub s: Option<String>,
    pub status: Option<String>,
    pub customer: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    #[serde(default)]
    pub tag: Vec<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

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
        let total = items.len();
        return Ok(Json(LoadListResponse { total, items }));
    }

    let (total, items) = state.db.list_loads(
        q.status.as_deref(), q.customer.as_deref(), &q.tag,
        q.from.as_deref(), q.to.as_deref(), limit, offset,
    ).await?;
    Ok(Json(LoadListResponse { total, items }))
}

pub async fn get_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_load_by_id(id).await?;
    let response = build_detail_response(&state, record).await?;
    Ok(Json(response))
}

pub async fn update_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateLoadRequest>,
) -> Result<impl IntoResponse, AppError> {
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

    let updated = state.db.update_load_metadata(
        id, body.customer_name, body.customer_ref, stops,
        body.rate_items, body.commodity, body.weight_lbs, body.miles,
        body.notes, body.tags, body.blob_ids, embedding,
    ).await?;

    if body.miles.is_none() {
        let _ = state.routing_tx.try_send(id);
    }

    let response = build_detail_response(&state, updated).await?;
    Ok(Json(response))
}

pub async fn delete_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    state.db.get_load_by_id(id).await?;
    state.db.delete_load_by_id(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn assign_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.transition_load_status(
        id, LoadStatus::Dispatched, None, None, None,
    ).await?;
    let response = build_detail_response(&state, record).await?;
    Ok(Json(response))
}

pub async fn dispatch_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.transition_load_status(
        id, LoadStatus::Dispatched, None, None, None,
    ).await?;
    let response = build_detail_response(&state, record).await?;
    Ok(Json(response))
}

pub async fn in_transit_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.transition_load_status(
        id, LoadStatus::InTransit, None, None, None,
    ).await?;
    let response = build_detail_response(&state, record).await?;
    Ok(Json(response))
}

pub async fn deliver_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.transition_load_status(
        id, LoadStatus::Delivered, None, None, None,
    ).await?;
    let response = build_detail_response(&state, record).await?;
    Ok(Json(response))
}

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

async fn resolve_stops(state: &AppState, inputs: Vec<StopInput>) -> Result<Vec<Stop>, AppError> {
    let mut stops = Vec::new();
    let mut resolutions: Vec<FacilityResolutionResponse> = Vec::new();

    for input in inputs {
        if !input.service_type.is_valid_for(&input.stop_type) {
            return Err(AppError::BadRequest(format!(
                "service_type '{}' is not valid for stop_type '{}'",
                input.service_type.as_str(), input.stop_type.as_str()
            )));
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
                Err(resolution) => {
                    resolutions.push(resolution);
                    continue;
                }
            }
        };

        stops.push(Stop {
            sequence: input.sequence,
            stop_type: input.stop_type,
            service_type: input.service_type,
            facility_id,
            scheduled_arrive: input.scheduled_arrive,
            notes: input.notes,
            blob_ids: input.blob_ids,
        });
    }

    if !resolutions.is_empty() {
        return Err(AppError::FacilityResolution(Box::new(resolutions.remove(0))));
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
            notes: stop.notes.clone(),
            blob_ids: stop.blob_ids.clone(),
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
