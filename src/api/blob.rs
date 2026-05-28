// src/api/blob.rs
use crate::{
    ai::extract::{extract_content, Extractable},
    api::utils::sanitize_filename,
    error::AppError,
    models::{BlobStatus, UpdateBlobRequest},
    storage::extract_store::{delete_extract, read_extract, write_extract, ExtractForQuery},
    AppState,
};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use utoipa::ToSchema;
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
    let disposition = format!("attachment; filename=\"{}\"", sanitize_filename(&record.name));

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
        let extract_base = std::path::Path::new(&state.config.extract_store_path);
        if let Err(e) = delete_extract(extract_base, &record.checksum).await {
            tracing::warn!("failed to delete extract cache for {}: {e}", record.checksum);
        }
    }
    state.db.delete_by_id(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BlobQueryRequest {
    /// The question to ask about the document (1–4096 characters)
    pub prompt: String,
    /// Ollama model to use (defaults to OLLAMA_SUMMARY_MODEL)
    pub model: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BlobQueryResponse {
    pub id: Uuid,
    pub prompt: String,
    pub answer: String,
    pub model: String,
    pub processing_time_ms: u64,
}

#[utoipa::path(
    post,
    path = "/api/v1/blobs/{id}/query",
    params(
        ("id" = Uuid, Path, description = "Blob UUID")
    ),
    request_body(content = BlobQueryRequest, description = "Prompt to ask about the document"),
    responses(
        (status = 200, description = "Answer from the LLM", body = BlobQueryResponse),
        (status = 400, description = "Invalid prompt (empty or too long)"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Blob not found"),
        (status = 422, description = "Blob not ready or content type not queryable"),
        (status = 500, description = "LLM inference error"),
    ),
    security(("BearerAuth" = [])),
    tag = "blobs"
)]
pub async fn query_blob(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<BlobQueryRequest>,
) -> Result<impl IntoResponse, AppError> {
    if body.prompt.is_empty() {
        return Err(AppError::BadRequest("prompt must not be empty".into()));
    }
    if body.prompt.len() > 4096 {
        return Err(AppError::BadRequest("prompt exceeds 4096 characters".into()));
    }

    let record = state.db.get_by_id(id).await?;
    if record.status != BlobStatus::Ready {
        return Err(AppError::UnprocessableEntity(
            "blob is not ready — wait for processing to complete".into(),
        ));
    }

    let extract_base = &state.config.extract_store_path;
    let model = body.model.unwrap_or_else(|| state.ai.summary_model.clone());

    let cached = read_extract(extract_base, &record.checksum).await;

    let extract = match cached {
        Some(e) => e,
        None => {
            // Cache miss: re-extract from blob bytes and write cache lazily
            let data = state.store.read(&record.checksum).await?;
            let extractable = extract_content(&data, &record.mime_type);
            if let Err(e) = write_extract(extract_base, &record.checksum, &extractable).await {
                tracing::warn!("lazy cache write failed for {id}: {e}");
            }
            match extractable {
                Extractable::Text(text) => ExtractForQuery::Text(text),
                Extractable::ScannedPdf(_, raw_text) => ExtractForQuery::ScannedPdf(raw_text),
                Extractable::ImageBytes(_) | Extractable::Unsupported => {
                    return Err(AppError::UnprocessableEntity(
                        "blob content type is not queryable".into(),
                    ));
                }
            }
        }
    };

    let start = Instant::now();

    let answer = match extract {
        ExtractForQuery::Text(text) => {
            let truncated: String = text.chars().take(12000).collect();
            let prompt = format!(
                "You are reading a freight document. Answer based ONLY on the following text.\n\
                Document:\n{truncated}\n\nUser question: {}",
                body.prompt
            );
            state.ai.generate(&model, &prompt, None).await?
        }
        ExtractForQuery::ScannedPdf(raw_text) => {
            // Raw PDF bytes are not a decodable image and crash the Ollama
            // vision runner (#281); answer from the extracted text using the
            // text model instead of forwarding bytes to the vision model.
            let truncated: String = raw_text.chars().take(2000).collect();
            let prompt = format!(
                "You are reading a scanned freight document. The raw text extracted is provided \
                as the only context (may be garbled due to font encoding).\n\
                RAW TEXT:\n{truncated}\n\nUser question: {}",
                body.prompt
            );
            state.ai.generate(&model, &prompt, None).await?
        }
    };

    let processing_time_ms = start.elapsed().as_millis() as u64;

    Ok(Json(BlobQueryResponse {
        id,
        prompt: body.prompt,
        answer,
        model,
        processing_time_ms,
    }))
}

