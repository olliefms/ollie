// src/api/blobs.rs
use crate::{
    error::AppError,
    models::{BlobListResponse, BlobRecord, BlobStatus, BlobVisibility},
    AppState,
};
use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use axum_extra::extract::Query;
use bytes::Bytes;
use chrono::Utc;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Deserialize, Default, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
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

/// Multipart form fields for blob upload
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct BlobUploadRequest {
    /// File bytes (any content type)
    #[schema(format = Binary)]
    pub file: Vec<u8>,
    /// Optional display name; defaults to the uploaded filename
    pub name: Option<String>,
    /// JSON-encoded array of tag strings, e.g. `["invoice","2024"]`
    pub tags: Option<String>,
    /// Visibility — `private` (default) or `driver`
    #[schema(value_type = String, example = "private")]
    pub visibility: Option<BlobVisibility>,
}

/// Max bytes accepted by the presigned upload route. Shared so the route's
/// `DefaultBodyLimit` and the `max_bytes` advertised by `upload_blob`
/// cannot drift apart.
pub(crate) const PRESIGNED_UPLOAD_MAX_BYTES: usize = 50 * 1024 * 1024;

/// Shared blob ingest: content-addressed dedup, storage write, DB insert, and
/// pipeline enqueue. Used by the admin multipart upload, the dispatcher multipart
/// upload, and the dispatcher presigned-URL upload so all three share one code path.
///
/// Returns `201 Created` when an identical file (same SHA-256) was already stored
/// (AI output copied from the existing record), or `202 Accepted` for a new file
/// queued for processing.
pub(crate) async fn ingest_blob(
    state: &AppState,
    data: Bytes,
    mime_type: String,
    name: String,
    tags: Vec<String>,
    visibility: BlobVisibility,
) -> Result<(StatusCode, BlobRecord), AppError> {
    let checksum = crate::storage::compute_checksum(&data);
    let now = Utc::now();

    if state.store.exists(&checksum).await {
        let existing = state.db.get_one_by_checksum(&checksum).await?;
        let (summary, embedding, status) = match existing {
            Some(ref r) => (r.summary.clone(), r.embedding.clone(), BlobStatus::Ready),
            None => (None, None, BlobStatus::Pending),
        };
        let record = BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum, name, mime_type,
            size: data.len() as i64, status, error: None, summary, tags,
            embedding, created_at: now, updated_at: now,
            visibility, uploaded_by: None,
        };
        state.db.insert(&record).await?;
        if matches!(record.status, BlobStatus::Pending) {
            state.pipeline_tx.send(record.id).await
                .map_err(|e| AppError::Internal(e.to_string()))?;
        }
        Ok((StatusCode::CREATED, record))
    } else {
        state.store.write(&data).await?;
        let record = BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum, name, mime_type,
            size: data.len() as i64, status: BlobStatus::Pending, error: None,
            summary: None, tags, embedding: None, created_at: now, updated_at: now,
            visibility, uploaded_by: None,
        };
        state.db.insert(&record).await?;
        state.pipeline_tx.send(record.id).await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok((StatusCode::ACCEPTED, record))
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/blobs",
    request_body(
        content = BlobUploadRequest,
        content_type = "multipart/form-data",
        description = "File upload with optional name and tags"
    ),
    responses(
        (status = 201, description = "Blob deduplicated — record created, AI output copied from existing", body = BlobRecord),
        (status = 202, description = "Blob accepted — queued for AI processing", body = BlobRecord),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "blobs"
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

    while let Some(field) = multipart.next_field().await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "file" => {
                filename = field.file_name().map(|s| s.to_string());
                file_content_type = field.content_type().map(|s| s.to_string());
                file_bytes = Some(field.bytes().await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?);
            }
            "name" => {
                display_name = Some(field.text().await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?);
            }
            "tags" => {
                let raw = field.text().await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                tags = serde_json::from_str(&raw)
                    .map_err(|_| AppError::BadRequest("tags must be a JSON array of strings".into()))?;
            }
            "visibility" => {
                let raw = field.text().await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                visibility = Some(raw.parse::<BlobVisibility>()
                    .map_err(|e| AppError::BadRequest(format!("invalid visibility: {e}")))?);
            }
            _ => {}
        }
    }

    let data = file_bytes.ok_or_else(|| AppError::BadRequest("missing 'file' field".into()))?;
    let mime_type = file_content_type
        .or_else(|| filename.as_ref().and_then(|f| mime_guess::from_path(f).first().map(|m| m.to_string())))
        .or_else(|| display_name.as_ref().and_then(|n| mime_guess::from_path(n).first().map(|m| m.to_string())))
        .unwrap_or_else(|| mime_guess::mime::APPLICATION_OCTET_STREAM.to_string());
    let name = display_name.or(filename).unwrap_or_else(|| "unnamed".to_string());

    let (status_code, record) =
        ingest_blob(&state, data, mime_type, name, tags, visibility.unwrap_or_default()).await?;

    Ok((status_code, Json(record)))
}

#[utoipa::path(
    get,
    path = "/api/v1/blobs",
    params(ListQuery),
    responses(
        (status = 200, description = "List of blobs (or semantic search results when ?s= is provided)", body = BlobListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "blobs"
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
