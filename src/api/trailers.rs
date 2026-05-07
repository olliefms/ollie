// src/api/trailers.rs
use crate::{
    ai::embed::embed_text,
    error::AppError,
    models::{
        CreateTrailerRequest, TrailerListResponse, TrailerOwner, TrailerRecord, TrailerStatus,
        UpdateTrailerRequest,
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
pub struct ListTrailersQuery {
    /// Semantic search query — triggers vector search when present
    pub s: Option<String>,
    /// Filter by status (available, assigned, dispatched, out_of_service, inactive)
    pub status: Option<String>,
    /// Filter by owner (fleet, carrier, customer, other)
    pub owner: Option<String>,
    /// Maximum results (default 20, max 100)
    pub limit: Option<usize>,
    /// Pagination offset (default 0)
    pub offset: Option<usize>,
}

#[utoipa::path(
    post,
    path = "/api/v1/trailers",
    request_body(content = CreateTrailerRequest, description = "Trailer to create"),
    responses(
        (status = 201, description = "Created trailer record", body = TrailerRecord),
        (status = 400, description = "Bad request — owner_name required when owner is not fleet"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "trailers"
)]
pub async fn create_trailer(
    State(state): State<AppState>,
    Json(body): Json<CreateTrailerRequest>,
) -> Result<impl IntoResponse, AppError> {
    if body.owner != TrailerOwner::Fleet && body.owner_name.is_none() {
        return Err(AppError::BadRequest("owner_name is required when owner is not fleet".into()));
    }

    let now = Utc::now();

    let record = TrailerRecord {
        id: Uuid::new_v4(),
        unit_number: body.unit_number,
        owner: body.owner,
        owner_name: body.owner_name,
        year: body.year,
        make: body.make,
        trailer_type: body.trailer_type,
        length_ft: body.length_ft,
        vin: body.vin,
        plate: body.plate,
        plate_state: body.plate_state,
        status: TrailerStatus::Available,
        notes: body.notes,
        embedding: None,
        owner_id: 0,
        created_at: now,
        updated_at: now,
    };

    let embedding = embed_text(&state.ai, &record.embedding_text()).await.ok();
    let record = TrailerRecord { embedding, ..record };

    state.db.insert_trailer(&record).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(
    get,
    path = "/api/v1/trailers",
    params(ListTrailersQuery),
    responses(
        (status = 200, description = "List of trailers (or semantic search results when ?s= is provided)", body = TrailerListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "trailers"
)]
pub async fn list_trailers(
    State(state): State<AppState>,
    Query(q): Query<ListTrailersQuery>,
) -> Result<impl IntoResponse, AppError> {
    let limit = q.limit.unwrap_or(20).min(100);
    let offset = q.offset.unwrap_or(0);

    if let Some(query_text) = q.s {
        let embedding = embed_text(&state.ai, &query_text).await?;
        let items = state.db.search_trailers(
            embedding,
            q.status.as_deref(),
            q.owner.as_deref(),
            limit,
        ).await?;
        let returned = items.len();
        return Ok(Json(TrailerListResponse { returned, items }));
    }

    let (total, items) = state.db.list_trailers(
        q.status.as_deref(),
        q.owner.as_deref(),
        limit,
        offset,
    ).await?;
    Ok(Json(TrailerListResponse { returned: total, items }))
}

#[utoipa::path(
    get,
    path = "/api/v1/trailers/{id}",
    params(
        ("id" = Uuid, Path, description = "Trailer UUID")
    ),
    responses(
        (status = 200, description = "Trailer record", body = TrailerRecord),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "trailers"
)]
pub async fn get_trailer(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_trailer_by_id(id).await?;
    Ok(Json(record))
}

#[utoipa::path(
    put,
    path = "/api/v1/trailers/{id}",
    params(
        ("id" = Uuid, Path, description = "Trailer UUID")
    ),
    request_body(content = UpdateTrailerRequest, description = "Fields to update — all optional. Can set out_of_service or available; cannot manually set assigned or dispatched."),
    responses(
        (status = 200, description = "Updated trailer record", body = TrailerRecord),
        (status = 400, description = "Bad request — cannot manually set assigned or dispatched"),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "trailers"
)]
pub async fn update_trailer(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateTrailerRequest>,
) -> Result<impl IntoResponse, AppError> {
    if matches!(body.status, Some(TrailerStatus::Assigned) | Some(TrailerStatus::Dispatched)) {
        return Err(AppError::BadRequest("cannot manually set assigned or dispatched status".into()));
    }

    let updated = state.db.update_trailer_metadata(
        id,
        body.unit_number,
        body.owner,
        body.owner_name,
        body.year,
        body.make,
        body.trailer_type,
        body.length_ft,
        body.vin,
        body.plate,
        body.plate_state,
        body.notes,
    ).await?;

    let updated = if let Some(status) = body.status {
        state.db.update_trailer_status(id, status).await?
    } else {
        updated
    };

    if let Ok(embedding) = embed_text(&state.ai, &updated.embedding_text()).await {
        let _ = state.db.update_trailer_embedding(id, embedding).await;
    }

    Ok(Json(updated))
}

#[utoipa::path(
    delete,
    path = "/api/v1/trailers/{id}",
    params(
        ("id" = Uuid, Path, description = "Trailer UUID")
    ),
    responses(
        (status = 204, description = "Soft-deleted (status set to inactive)"),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "trailers"
)]
pub async fn delete_trailer(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    state.db.soft_delete_trailer(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn router() -> Router<AppState> {
    use axum::routing::{delete, get, post, put};
    Router::new()
        .route("/api/v1/trailers", post(create_trailer))
        .route("/api/v1/trailers", get(list_trailers))
        .route("/api/v1/trailers/:id", get(get_trailer))
        .route("/api/v1/trailers/:id", put(update_trailer))
        .route("/api/v1/trailers/:id", delete(delete_trailer))
}
