// src/api/trucks.rs
use crate::{
    ai::embed::embed_text,
    error::AppError,
    models::{
        CreateTruckRequest, TruckListResponse, TruckRecord, TruckStatus,
        UpdateTruckRequest,
    },
    AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
    Router,
};
use axum_extra::extract::Query;
use chrono::Utc;
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

#[derive(Deserialize, Default, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListTrucksQuery {
    /// Semantic search query — triggers vector search when present
    pub s: Option<String>,
    /// Filter by status (available, assigned, dispatched, out_of_service, inactive)
    pub status: Option<String>,
    /// Maximum results (default 20, max 100)
    pub limit: Option<usize>,
    /// Pagination offset (default 0)
    pub offset: Option<usize>,
}

#[utoipa::path(
    post,
    path = "/api/v1/trucks",
    request_body(content = CreateTruckRequest, description = "Truck to create"),
    responses(
        (status = 201, description = "Created truck record", body = TruckRecord),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "trucks"
)]
pub async fn create_truck(
    State(state): State<AppState>,
    Json(body): Json<CreateTruckRequest>,
) -> Result<impl IntoResponse, AppError> {
    let now = Utc::now();

    let record = TruckRecord {
        id: Uuid::new_v4(),
        unit_number: body.unit_number,
        year: body.year,
        make: body.make,
        model: body.model,
        vin: body.vin,
        plate: body.plate,
        plate_state: body.plate_state,
        status: TruckStatus::Available,
        notes: body.notes,
        blob_ids: body.blob_ids,
        embedding: None,
        owner_id: 0,
        created_at: now,
        updated_at: now,
    };

    let embedding = embed_text(&state.ai, &record.embedding_text()).await.ok();
    let record = TruckRecord { embedding, ..record };

    state.db.insert_truck(&record).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(
    get,
    path = "/api/v1/trucks",
    params(ListTrucksQuery),
    responses(
        (status = 200, description = "List of trucks (or semantic search results when ?s= is provided)", body = TruckListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "trucks"
)]
pub async fn list_trucks(
    State(state): State<AppState>,
    Query(q): Query<ListTrucksQuery>,
) -> Result<impl IntoResponse, AppError> {
    let limit = q.limit.unwrap_or(20).min(100);
    let offset = q.offset.unwrap_or(0);

    if let Some(query_text) = q.s {
        let embedding = embed_text(&state.ai, &query_text).await?;
        let items = state.db.search_trucks(embedding, q.status.as_deref(), limit).await?;
        let returned = items.len();
        return Ok(Json(TruckListResponse { returned, items }));
    }

    let (total, items) = state.db.list_trucks(q.status.as_deref(), limit, offset).await?;
    Ok(Json(TruckListResponse { returned: total, items }))
}

#[utoipa::path(
    get,
    path = "/api/v1/trucks/{id}",
    params(
        ("id" = Uuid, Path, description = "Truck UUID")
    ),
    responses(
        (status = 200, description = "Truck record", body = TruckRecord),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "trucks"
)]
pub async fn get_truck(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_truck_by_id(id).await?;
    Ok(Json(record))
}

#[utoipa::path(
    put,
    path = "/api/v1/trucks/{id}",
    params(
        ("id" = Uuid, Path, description = "Truck UUID")
    ),
    request_body(content = UpdateTruckRequest, description = "Fields to update — all optional. Can set out_of_service or available; cannot manually set assigned or dispatched."),
    responses(
        (status = 200, description = "Updated truck record", body = TruckRecord),
        (status = 400, description = "Bad request — cannot manually set assigned or dispatched"),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "trucks"
)]
pub async fn update_truck(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateTruckRequest>,
) -> Result<impl IntoResponse, AppError> {
    if matches!(body.status, Some(TruckStatus::Assigned) | Some(TruckStatus::Dispatched)) {
        return Err(AppError::BadRequest("cannot manually set assigned or dispatched status".into()));
    }

    let updated = state.db.update_truck_metadata(
        id,
        body.unit_number,
        body.year,
        body.make,
        body.model,
        body.vin,
        body.plate,
        body.plate_state,
        body.notes,
        body.blob_ids,
    ).await?;

    let updated = if let Some(status) = body.status {
        state.db.update_truck_status(id, status).await?
    } else {
        updated
    };

    if let Ok(embedding) = embed_text(&state.ai, &updated.embedding_text()).await {
        let _ = state.db.update_truck_embedding(id, embedding).await;
    }

    Ok(Json(updated))
}

#[utoipa::path(
    delete,
    path = "/api/v1/trucks/{id}",
    params(
        ("id" = Uuid, Path, description = "Truck UUID")
    ),
    responses(
        (status = 204, description = "Soft-deleted (status set to inactive)"),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "trucks"
)]
pub async fn delete_truck(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    state.db.soft_delete_truck(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn router() -> Router<AppState> {
    use axum::routing::{delete, get, post, put};
    Router::new()
        .route("/api/v1/trucks", post(create_truck))
        .route("/api/v1/trucks", get(list_trucks))
        .route("/api/v1/trucks/{id}", get(get_truck))
        .route("/api/v1/trucks/{id}", put(update_truck))
        .route("/api/v1/trucks/{id}", delete(delete_truck))
}
