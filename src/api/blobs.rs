// src/api/blobs.rs
use crate::{
    error::AppError,
    models::{BlobListResponse, BlobRecord, BlobStatus},
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
use uuid::Uuid;

#[derive(Deserialize, Default)]
pub struct ListQuery {
    pub s: Option<String>,
    pub name: Option<String>,
    // axum_extra::Query handles repeated ?tag=a&tag=b correctly
    #[serde(default)]
    pub tag: Vec<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

pub async fn upload_blob(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, AppError> {
    let mut file_bytes: Option<Bytes> = None;
    let mut filename: Option<String> = None;
    let mut file_content_type: Option<String> = None;
    let mut display_name: Option<String> = None;
    let mut tags: Vec<String> = vec![];

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
            _ => {}
        }
    }

    let data = file_bytes.ok_or_else(|| AppError::BadRequest("missing 'file' field".into()))?;
    let mime_type = file_content_type
        .or_else(|| filename.as_ref().and_then(|f| mime_guess::from_path(f).first().map(|m| m.to_string())))
        .or_else(|| display_name.as_ref().and_then(|n| mime_guess::from_path(n).first().map(|m| m.to_string())))
        .unwrap_or_else(|| mime_guess::mime::APPLICATION_OCTET_STREAM.to_string());
    let name = display_name.or(filename).unwrap_or_else(|| "unnamed".to_string());
    let checksum = crate::storage::compute_checksum(&data);
    let now = Utc::now();

    let (status_code, record) = if state.store.exists(&checksum).await {
        let existing = state.db.get_one_by_checksum(&checksum).await?;
        let (summary, embedding, status) = match existing {
            Some(ref r) => (r.summary.clone(), r.embedding.clone(), BlobStatus::Ready),
            None => (None, None, BlobStatus::Pending),
        };
        let record = BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum, name, mime_type,
            size: data.len() as i64, status, error: None, summary, tags,
            embedding, created_at: now, updated_at: now,
        };
        state.db.insert(&record).await?;
        if matches!(record.status, BlobStatus::Pending) {
            state.pipeline_tx.send(record.id).await
                .map_err(|e| AppError::Internal(e.to_string()))?;
        }
        (StatusCode::CREATED, record)
    } else {
        state.store.write(&data).await?;
        let record = BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum, name, mime_type,
            size: data.len() as i64, status: BlobStatus::Pending, error: None,
            summary: None, tags, embedding: None, created_at: now, updated_at: now,
        };
        state.db.insert(&record).await?;
        state.pipeline_tx.send(record.id).await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        (StatusCode::ACCEPTED, record)
    };

    Ok((status_code, Json(record)))
}

pub async fn list_blobs(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<impl IntoResponse, AppError> {
    let limit = q.limit.unwrap_or(20).min(100);
    let offset = q.offset.unwrap_or(0);

    if let Some(query_text) = q.s {
        let embedding = crate::ai::embed::embed_text(&state.ai, &query_text).await?;
        let items = state.db.search(embedding, q.name.as_deref(), &q.tag, limit).await?;
        let total = items.len();
        return Ok(Json(BlobListResponse { total, items }));
    }

    let (total, items) = state.db.list(q.name.as_deref(), &q.tag, limit, offset).await?;
    Ok(Json(BlobListResponse { total, items }))
}
