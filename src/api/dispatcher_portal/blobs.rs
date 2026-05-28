// src/api/dispatcher_portal/blobs.rs
//
// Dispatcher portal blob endpoints. Auth enforced at router layer via
// require_dispatcher_jwt. Handlers delegate to the same DB ops, storage, and
// AI layer used by the admin blob API.

use crate::{
    ai::extract::{bytes_to_base64, extract_content, Extractable},
    api::blob::{BlobQueryRequest, BlobQueryResponse},
    api::blobs::ListQuery,
    api::utils::sanitize_filename,
    error::AppError,
    models::{BlobListResponse, BlobStatus, BlobVisibility, UpdateBlobRequest},
    storage::extract_store::{delete_extract, read_extract, write_extract, ExtractForQuery},
    AppState,
};
use axum::{
    body::Body,
    extract::{Multipart, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use axum_extra::extract::Query;
use bytes::Bytes;
use serde::Deserialize;
use std::time::Instant;
use uuid::Uuid;

use super::blob_links::{self, BlobUrlOp};

#[utoipa::path(
    get,
    path = "/dispatch/api/v1/blobs",
    params(ListQuery),
    responses(
        (status = 200, description = "List of blobs (or semantic search results when ?s= provided)", body = BlobListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn list_blobs(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<impl IntoResponse, AppError> {
    let limit = q.limit.unwrap_or(20).min(100);
    let offset = q.offset.unwrap_or(0);

    if let Some(query_text) = q.s {
        let embedding = crate::ai::embed::embed_text(&state.ai, &query_text).await?;
        let items = state.db.search(embedding, q.name.as_deref(), &q.tag, limit).await?;
        let returned = items.len();
        return Ok(Json(BlobListResponse { returned, items }));
    }

    let (_total, items) = state.db.list(q.name.as_deref(), &q.tag, limit, offset).await?;
    let returned = items.len();
    Ok(Json(BlobListResponse { returned, items }))
}

#[utoipa::path(
    post,
    path = "/dispatch/api/v1/blobs",
    request_body(
        content = crate::api::blobs::BlobUploadRequest,
        content_type = "multipart/form-data",
        description = "File upload with optional name and tags"
    ),
    responses(
        (status = 201, description = "Blob deduplicated — AI output copied from existing", body = BlobRecord),
        (status = 202, description = "Blob accepted — queued for AI processing", body = BlobRecord),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
)]
pub async fn upload_blob(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, AppError> {
    let mut file_bytes: Option<Bytes> = None;
    let mut filename: Option<String> = None;
    let mut file_content_type: Option<String> = None;
    let mut display_name: Option<String> = None;
    let mut tags: Vec<String> = vec![];
    let mut visibility: Option<BlobVisibility> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "file" => {
                filename = field.file_name().map(|s| s.to_string());
                file_content_type = field.content_type().map(|s| s.to_string());
                file_bytes = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| AppError::BadRequest(e.to_string()))?,
                );
            }
            "name" => {
                display_name = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError::BadRequest(e.to_string()))?,
                );
            }
            "tags" => {
                let raw = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                tags = serde_json::from_str(&raw)
                    .map_err(|_| AppError::BadRequest("tags must be a JSON array of strings".into()))?;
            }
            "visibility" => {
                let raw = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                visibility = Some(
                    raw.parse::<BlobVisibility>()
                        .map_err(|e| AppError::BadRequest(format!("invalid visibility: {e}")))?,
                );
            }
            _ => {}
        }
    }

    let data = file_bytes.ok_or_else(|| AppError::BadRequest("missing 'file' field".into()))?;
    let mime_type = file_content_type
        .or_else(|| {
            filename
                .as_ref()
                .and_then(|f| mime_guess::from_path(f).first().map(|m| m.to_string()))
        })
        .or_else(|| {
            display_name
                .as_ref()
                .and_then(|n| mime_guess::from_path(n).first().map(|m| m.to_string()))
        })
        .unwrap_or_else(|| mime_guess::mime::APPLICATION_OCTET_STREAM.to_string());
    let name = display_name
        .or(filename)
        .unwrap_or_else(|| "unnamed".to_string());

    let (status_code, record) = crate::api::blobs::ingest_blob(
        &state, data, mime_type, name, tags, visibility.unwrap_or_default(),
    ).await?;

    Ok((status_code, Json(record)))
}

#[utoipa::path(
    get,
    path = "/dispatch/api/v1/blob/{id}",
    params(("id" = Uuid, Path, description = "Blob UUID")),
    responses(
        (status = 200, description = "Blob record (Accept: application/json) or raw file bytes", body = BlobRecord),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
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
    path = "/dispatch/api/v1/blob/{id}",
    params(("id" = Uuid, Path, description = "Blob UUID")),
    request_body(content = UpdateBlobRequest, description = "Fields to update — at least one of name or tags required"),
    responses(
        (status = 200, description = "Updated blob record", body = BlobRecord),
        (status = 400, description = "Bad request — neither name nor tags provided"),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
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
    path = "/dispatch/api/v1/blob/{id}",
    params(("id" = Uuid, Path, description = "Blob UUID")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 409, description = "Conflict — blob is referenced by one or more loads"),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
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

#[utoipa::path(
    post,
    path = "/dispatch/api/v1/blobs/{id}/query",
    params(("id" = Uuid, Path, description = "Blob UUID")),
    request_body(content = BlobQueryRequest, description = "Prompt to ask about the document"),
    responses(
        (status = 200, description = "Answer from the LLM", body = BlobQueryResponse),
        (status = 400, description = "Invalid prompt (empty or too long)"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Blob not found"),
        (status = 422, description = "Blob not ready or content type not queryable"),
    ),
    security(("BearerAuth" = [])),
    tag = "dispatch"
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
            let data = state.store.read(&record.checksum).await?;
            let b64 = bytes_to_base64(&data);
            let truncated: String = raw_text.chars().take(2000).collect();
            let vision_model = state.ai.vision_model.clone();
            let prompt = format!(
                "You are reading a scanned freight document. The raw text extracted is provided \
                as auxiliary context (may be garbled due to font encoding).\n\
                RAW TEXT:\n{truncated}\n\nUser question: {}",
                body.prompt
            );
            state.ai.generate(&vision_model, &prompt, Some(b64)).await?
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

// ---------------------------------------------------------------------------
// Presigned (token-authenticated) blob byte transfer.
//
// These two routes are mounted OUTSIDE the dispatcher JWT middleware. They are
// authenticated by a short-lived, blob-scoped token in the `token` query param
// (minted by the MCP tools `upload_blob` / `get_blob_url`), letting an
// agent that holds no dispatcher JWT move file bytes over plain HTTP without
// putting large payloads on the MCP transport. See `blob_links.rs`.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct PresignedDownloadQuery {
    /// Presigned token from `get_blob_url`.
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct PresignedUploadQuery {
    /// Presigned token from `upload_blob`.
    pub token: String,
    /// Optional display name; defaults to "unnamed". MIME is inferred from it when
    /// the `Content-Type` header is absent.
    pub name: Option<String>,
    /// Optional comma-separated tags, e.g. `tags=invoice,2026`.
    pub tags: Option<String>,
}

#[utoipa::path(
    post,
    path = "/dispatch/blobs/presigned",
    params(
        ("token" = String, Query, description = "Presigned upload token from upload_blob"),
        ("name" = Option<String>, Query, description = "Display name; MIME inferred from it if Content-Type header absent"),
        ("tags" = Option<String>, Query, description = "Comma-separated tags")
    ),
    request_body(content = String, description = "Raw file bytes; set Content-Type to the file's MIME type", content_type = "application/octet-stream"),
    responses(
        (status = 201, description = "Deduplicated — record created, AI output copied", body = BlobRecord),
        (status = 202, description = "Accepted — queued for AI processing", body = BlobRecord),
        (status = 400, description = "Empty body"),
        (status = 401, description = "Missing, invalid, expired, or wrong-scope token"),
    ),
    tag = "dispatch"
)]
pub async fn presigned_upload(
    State(state): State<AppState>,
    Query(q): Query<PresignedUploadQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    blob_links::verify_token(&state.config.dispatcher_jwt_secret, &q.token, BlobUrlOp::Post)?;

    if body.is_empty() {
        return Err(AppError::BadRequest("request body is empty".into()));
    }

    let mime_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| q.name.as_ref().and_then(|n| mime_guess::from_path(n).first().map(|m| m.to_string())))
        .unwrap_or_else(|| mime_guess::mime::APPLICATION_OCTET_STREAM.to_string());
    let name = q.name.clone().unwrap_or_else(|| "unnamed".to_string());
    let tags: Vec<String> = q
        .tags
        .as_deref()
        .map(|s| s.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect())
        .unwrap_or_default();

    let (status_code, record) =
        crate::api::blobs::ingest_blob(&state, body, mime_type, name, tags, BlobVisibility::Private).await?;
    Ok((status_code, Json(record)))
}

#[utoipa::path(
    get,
    path = "/dispatch/blobs/presigned/{id}",
    params(
        ("id" = Uuid, Path, description = "Blob UUID"),
        ("token" = String, Query, description = "Presigned download token from get_blob_url")
    ),
    responses(
        (status = 200, description = "Raw file bytes"),
        (status = 401, description = "Missing, invalid, expired, wrong-scope, or id-mismatched token"),
        (status = 404, description = "Not found"),
    ),
    tag = "dispatch"
)]
pub async fn presigned_download(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<PresignedDownloadQuery>,
) -> Result<impl IntoResponse, AppError> {
    let claims = blob_links::verify_token(&state.config.dispatcher_jwt_secret, &q.token, BlobUrlOp::Get)?;
    // Token is bound to a single blob — reject if the path id doesn't match.
    if claims.sub != id.to_string() {
        return Err(AppError::Unauthorized);
    }

    let record = state.db.get_by_id(id).await?;
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

