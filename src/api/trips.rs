// src/api/trips.rs
use crate::{
    ai::embed::embed_text,
    error::AppError,
    models::{
        load::StopType,
        trip::{CreateTripRequest, TripListResponse, TripRecord, TripStatus, TripStop, TripStopType, UpdateTripRequest},
    },
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

    let stops = if body.stops.is_empty() {
        if let Some(load_id) = body.load_id {
            let load = state.db.get_load_by_id(load_id).await?;
            load.stops.into_iter().enumerate().map(|(idx, s)| TripStop {
                sequence: s.sequence,
                stop_type: match s.stop_type {
                    StopType::Pickup => TripStopType::Pickup,
                    StopType::Delivery => TripStopType::Delivery,
                },
                facility_id: Some(s.facility_id),
                name: None,
                address: None,
                load_stop_index: Some(idx as u32),
                scheduled_arrive: Some(s.scheduled_arrive),
                scheduled_arrive_end: s.scheduled_arrive_end,
                actual_arrive: None,
                actual_depart: None,
                expected_dwell_minutes: s.expected_dwell_minutes,
                detention_free_minutes: s.detention_free_minutes,
                detention_grace_minutes: s.detention_grace_minutes,
                notes: s.notes,
                timezone: s.timezone,
            }).collect()
        } else {
            body.stops
        }
    } else {
        body.stops
    };

    let mut record = TripRecord {
        id: Uuid::new_v4(),
        trip_number,
        load_id: body.load_id,
        load_number: None,
        previous_trip_id: None,
        deadhead_miles: None,
        loaded_miles: None,
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

    let items = state.db.list_trips(
        q.load_id, q.driver_id, q.status.as_deref(),
    ).await?;
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
    let record = state.db.get_trip(id).await?;
    Ok(Json(record))
}

#[utoipa::path(
    put,
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

    let record = state.db.update_trip_metadata(
        id, body.load_id, body.sequence, body.stops, body.notes, embedding,
    ).await?;
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
