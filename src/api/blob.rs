// src/api/blob.rs
use crate::{error::AppError, models::UpdateBlobRequest, AppState};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use uuid::Uuid;

#[utoipa::path(
    get,
    path = "/api/v1/blob/{id}",
    params(
        ("id" = Uuid, Path, description = "Blob UUID")
    ),
    responses(
        (status = 200, description = "Blob record (when Accept: application/json) or raw file bytes", body = crate::models::BlobRecord),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "blobs"
)]
pub async fn get_blob(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_by_id(id).await?;

    let wants_json = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("application/json"))
        .unwrap_or(false);

    if wants_json {
        return Ok(Json(record).into_response());
    }

    // File is always on disk once the blob record exists
    let data = state.store.read(&record.checksum).await?;
    let disposition = format!("attachment; filename=\"{}\"", record.name);

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, record.mime_type),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        Body::from(data),
    )
        .into_response())
}

#[utoipa::path(
    put,
    path = "/api/v1/blob/{id}",
    params(
        ("id" = Uuid, Path, description = "Blob UUID")
    ),
    request_body(content = UpdateBlobRequest, description = "Fields to update — at least one of name or tags required"),
    responses(
        (status = 200, description = "Updated blob record", body = crate::models::BlobRecord),
        (status = 400, description = "Bad request — neither name nor tags provided"),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "blobs"
)]
pub async fn update_blob(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateBlobRequest>,
) -> Result<impl IntoResponse, AppError> {
    if body.name.is_none() && body.tags.is_none() {
        return Err(AppError::BadRequest(
            "at least one of 'name' or 'tags' is required".into(),
        ));
    }
    let updated = state.db.update_metadata(id, body.name, body.tags).await?;
    Ok(Json(updated))
}

#[utoipa::path(
    delete,
    path = "/api/v1/blob/{id}",
    params(
        ("id" = Uuid, Path, description = "Blob UUID")
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 409, description = "Conflict — blob is referenced by one or more loads"),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "blobs"
)]
pub async fn delete_blob(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_by_id(id).await?;

    if state.db.any_load_references_blob(id).await? {
        return Err(AppError::Conflict(
            "blob is referenced by one or more loads and cannot be deleted".into(),
        ));
    }

    let ref_count = state.db.count_by_checksum(&record.checksum).await?;
    if ref_count <= 1 {
        state.store.delete(&record.checksum).await?;
    }
    state.db.delete_by_id(id).await?;
    Ok(StatusCode::NO_CONTENT)
}
