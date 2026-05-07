// src/api/facilities.rs
use crate::{
    ai::embed::embed_text,
    error::AppError,
    models::{
        CreateFacilityRequest, FacilityListResponse, FacilityRecord,
        FacilityResolutionResponse, GeocodeStatus, UpdateFacilityRequest,
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
use utoipa::IntoParams;
use uuid::Uuid;

#[derive(Deserialize, Default, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListFacilitiesQuery {
    /// Semantic search query — triggers vector search when present
    pub s: Option<String>,
    /// Filter by name (substring match)
    pub name: Option<String>,
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
    path = "/api/v1/facilities",
    request_body(content = CreateFacilityRequest, description = "Facility to create"),
    responses(
        (status = 201, description = "Created facility record", body = FacilityRecord),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "facilities"
)]
pub async fn create_facility(
    State(state): State<AppState>,
    Json(body): Json<CreateFacilityRequest>,
) -> Result<impl IntoResponse, AppError> {
    let now = Utc::now();
    let embedding_text = format!(
        "{} {} {} {}",
        body.name,
        body.address,
        body.notes.as_deref().unwrap_or(""),
        body.tags.join(" "),
    );

    let embedding = embed_text(&state.ai, &embedding_text).await.ok();

    let record = FacilityRecord {
        id: Uuid::new_v4(), owner_id: 0,
        name: body.name, address: body.address,
        normalized_address: None, lat: None, lng: None,
        geocode_status: GeocodeStatus::Pending, geocode_failure_count: 0,
        contacts: body.contacts,
        notes: body.notes, tags: body.tags, blob_ids: body.blob_ids,
        avg_dwell_minutes: None, dwell_sample_count: 0,
        embedding, created_at: now, updated_at: now,
    };

    state.db.insert_facility(&record).await?;
    let _ = state.geocoding_tx.try_send(record.id);

    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(
    get,
    path = "/api/v1/facilities",
    params(ListFacilitiesQuery),
    responses(
        (status = 200, description = "List of facilities (or semantic search results when ?s= is provided)", body = FacilityListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "facilities"
)]
pub async fn list_facilities(
    State(state): State<AppState>,
    Query(q): Query<ListFacilitiesQuery>,
) -> Result<impl IntoResponse, AppError> {
    let limit = q.limit.unwrap_or(20).min(100);
    let offset = q.offset.unwrap_or(0);

    if let Some(query_text) = q.s {
        let embedding = embed_text(&state.ai, &query_text).await?;
        let items = state.db.search_facilities(embedding, q.name.as_deref(), &q.tag, limit).await?;
        let returned = items.len();
        return Ok(Json(FacilityListResponse { returned, items }));
    }

    let (_total, items) = state.db.list_facilities(q.name.as_deref(), &q.tag, limit, offset).await?;
    let returned = items.len();
    Ok(Json(FacilityListResponse { returned, items }))
}

#[utoipa::path(
    get,
    path = "/api/v1/facilities/{id}",
    params(
        ("id" = Uuid, Path, description = "Facility UUID")
    ),
    responses(
        (status = 200, description = "Facility record", body = FacilityRecord),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "facilities"
)]
pub async fn get_facility(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_facility_by_id(id).await?;
    Ok(Json(record))
}

#[utoipa::path(
    patch,
    path = "/api/v1/facilities/{id}",
    params(
        ("id" = Uuid, Path, description = "Facility UUID")
    ),
    request_body(content = UpdateFacilityRequest, description = "Fields to update — all optional"),
    responses(
        (status = 200, description = "Updated facility record", body = FacilityRecord),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "facilities"
)]
pub async fn update_facility(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateFacilityRequest>,
) -> Result<impl IntoResponse, AppError> {
    let address_changed = body.address.is_some();
    let updated = state.db.update_facility_metadata(
        id, body.name, body.address, body.contacts,
        body.notes, body.tags, body.blob_ids,
    ).await?;

    let embedding_text = updated.embedding_text();
    if let Ok(embedding) = embed_text(&state.ai, &embedding_text).await {
        let _ = state.db.update_facility_embedding(id, embedding).await;
    }

    if address_changed {
        let _ = state.geocoding_tx.try_send(id);
    }

    Ok(Json(updated))
}

#[utoipa::path(
    delete,
    path = "/api/v1/facilities/{id}",
    params(
        ("id" = Uuid, Path, description = "Facility UUID")
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 409, description = "Conflict — facility is referenced by one or more loads"),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "facilities"
)]
pub async fn delete_facility(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    state.db.get_facility_by_id(id).await?;

    if state.db.any_load_references_facility(id).await? {
        return Err(AppError::Conflict(
            "facility is referenced by one or more loads and cannot be deleted".into(),
        ));
    }

    state.db.delete_facility_by_id(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Resolve a facility from a name+address string, applying dedup logic.
/// Returns Ok(Uuid) if resolved/created.
/// Returns Err(AppError::FacilityResolution) if ambiguous (stop_index defaults to 0; caller overrides).
/// Returns Err on embed or DB failure — fails closed rather than silently creating duplicates.
pub async fn resolve_or_create_facility(
    state: &AppState,
    name: &str,
    address: &str,
    force_new: bool,
) -> Result<Uuid, AppError> {
    if force_new {
        return create_new_facility(state, name, address).await;
    }

    let text = format!("{name} {address}");
    let embedding = embed_text(&state.ai, &text).await?;

    let candidates = state.db.search_facilities(embedding, None, &[], 5).await?;

    let high = state.config.facility_dedup_high_threshold as f32;
    let low = state.config.facility_dedup_low_threshold as f32;

    if let Some(top) = candidates.first() {
        if top.score.unwrap_or(0.0) >= high {
            return Ok(top.id);
        }
    }

    let above_low: Vec<_> = candidates.into_iter()
        .filter(|c| c.score.unwrap_or(0.0) >= low)
        .map(|c| crate::models::FacilityCandidate {
            id: c.id, name: c.name, address: c.address,
            normalized_address: c.normalized_address,
            score: c.score.unwrap_or(0.0),
        })
        .collect();

    if !above_low.is_empty() {
        return Err(AppError::FacilityResolution(Box::new(vec![FacilityResolutionResponse {
            stop_index: 0,
            facility_resolution_required: true,
            candidates: above_low,
        }])));
    }

    create_new_facility(state, name, address).await
}

async fn create_new_facility(
    state: &AppState,
    name: &str,
    address: &str,
) -> Result<Uuid, AppError> {
    let now = Utc::now();
    let text = format!("{name} {address}");
    let embedding = embed_text(&state.ai, &text).await.ok();
    let record = FacilityRecord {
        id: Uuid::new_v4(), owner_id: 0,
        name: name.to_string(), address: address.to_string(),
        normalized_address: None, lat: None, lng: None,
        geocode_status: GeocodeStatus::Pending, geocode_failure_count: 0,
        contacts: vec![], notes: None, tags: vec![], blob_ids: vec![],
        avg_dwell_minutes: None, dwell_sample_count: 0,
        embedding, created_at: now, updated_at: now,
    };
    state.db.insert_facility(&record).await?;
    let _ = state.geocoding_tx.try_send(record.id);
    Ok(record.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ai::OllamaClient, config::Config, db::DbClient, routing::RoutingClient,
        storage::BlobStore,
    };
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn test_state() -> (AppState, TempDir, TempDir) {
        let blob_dir = TempDir::new().unwrap();
        let db_dir = TempDir::new().unwrap();
        std::env::set_var("ADMIN_API_KEY", "test-secret");
        let config = Arc::new(Config::from_env().unwrap());
        let db = Arc::new(DbClient::new(db_dir.path().to_str().unwrap(), 4).await.unwrap());
        let store = Arc::new(BlobStore::new(blob_dir.path().to_str().unwrap()));
        let ai = Arc::new(OllamaClient::new(
            "http://localhost:11434", "nomic-embed-text", "llama3.2", "llava",
        ));
        let geocoding = Arc::new(crate::geocoding::GeocodingClient::new());
        let ors = Arc::new(RoutingClient::new(""));
        let (geocoding_tx, _rx) = async_channel::bounded(10);
        let (routing_tx, _rx2) = async_channel::bounded(10);
        let (pipeline_tx, _rx3) = async_channel::bounded(10);
        let state = AppState { db, store, ai, geocoding, ors, pipeline_tx, geocoding_tx, routing_tx, config };
        (state, blob_dir, db_dir)
    }

    #[tokio::test]
    async fn test_resolve_force_new_creates_facility() {
        let (state, _b, _d) = test_state().await;
        let id = resolve_or_create_facility(&state, "Fresh Dock", "123 Main St, Dallas TX", true)
            .await
            .expect("force_new should create a new facility even without Ollama");
        state.db.get_facility_by_id(id).await.expect("facility should exist");
    }

    #[tokio::test]
    async fn test_resolve_propagates_error_when_embed_fails() {
        let (state, _b, _d) = test_state().await;
        // Without Ollama, embed_text fails and the error is propagated (fail closed)
        let result = resolve_or_create_facility(&state, "Dock A", "100 Oak Ave, Nashville TN", false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_resolve_force_new_skips_dedup() {
        let (state, _b, _d) = test_state().await;
        let id1 = resolve_or_create_facility(&state, "Dock B", "200 Elm St, Atlanta GA", true).await.unwrap();
        let id2 = resolve_or_create_facility(&state, "Dock B", "200 Elm St, Atlanta GA", true).await.unwrap();
        assert_ne!(id1, id2, "force_new_facility=true must always create a new record");
    }
}
