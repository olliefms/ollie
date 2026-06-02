// src/api/blobs.rs
use crate::{
    error::AppError,
    models::{BlobRecord, BlobStatus, BlobVisibility},
    AppState,
};
use axum::http::StatusCode;
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
/// pipeline enqueue. Used by the admin multipart upload, the fleet_user multipart
/// upload, and the fleet_user presigned-URL upload so all three share one code path.
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
