// src/api/driver_portal/documents.rs
use crate::{
    api::driver_portal::jwt::DriverClaims,
    api::utils::sanitize_filename,
    error::AppError,
    models::{BlobRecord, BlobStatus, BlobVisibility, ExpenseCategory},
    AppState,
};
use axum::{
    body::Body,
    extract::{Multipart, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    Extension, Json,
};
use bytes::Bytes;
use chrono::Utc;
use uuid::Uuid;

const MAX_DOC_SIZE: usize = 50 * 1024 * 1024;
const ALLOWED_DOCTYPES: &[&str] = &["bol", "pod", "scale_ticket", "expense", "other"];

async fn assert_trip_belongs_to_driver(
    state: &AppState,
    trip_id: Uuid,
    driver_id: Uuid,
) -> Result<(), AppError> {
    load_id_for_driver_trip(state, trip_id, driver_id).await.map(|_| ())
}

async fn load_id_for_driver_trip(
    state: &AppState,
    trip_id: Uuid,
    driver_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    let trip = state.db.get_trip(trip_id).await?;
    if trip.driver_id != Some(driver_id) {
        return Err(AppError::NotFound);
    }
    Ok(trip.load_id)
}

#[utoipa::path(
    post,
    path = "/driver/api/v1/trips/{id}/documents",
    request_body(
        content = crate::api::blobs::BlobUploadRequest,
        content_type = "multipart/form-data",
        description = "Multipart file upload with optional doctype"
    ),
    responses(
        (status = 201, description = "Document deduplicated", body = BlobRecord),
        (status = 202, description = "Document accepted and queued", body = BlobRecord),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Trip not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "driver"
)]
pub async fn upload_document(
    State(state): State<AppState>,
    Extension(claims): Extension<DriverClaims>,
    Path(trip_id): Path<Uuid>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, AppError> {
    let driver_id = claims
        .driver_id
        .parse::<Uuid>()
        .map_err(|_| AppError::Unauthorized)?;
    let load_id = load_id_for_driver_trip(&state, trip_id, driver_id).await?;

    let mut file_bytes: Option<Bytes> = None;
    let mut filename: Option<String> = None;
    let mut file_content_type: Option<String> = None;
    let mut doctype = "other".to_string();
    let mut expense_category: Option<ExpenseCategory> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        match field.name().unwrap_or("") {
            "file" => {
                filename = field.file_name().map(|s| s.to_string());
                file_content_type = field.content_type().map(|s| s.to_string());
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                if bytes.len() > MAX_DOC_SIZE {
                    return Err(AppError::BadRequest(format!(
                        "file exceeds {} bytes",
                        MAX_DOC_SIZE
                    )));
                }
                file_bytes = Some(bytes);
            }
            "doctype" => {
                let raw = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                if !ALLOWED_DOCTYPES.contains(&raw.as_str()) {
                    return Err(AppError::BadRequest(format!("invalid doctype: {raw}")));
                }
                doctype = raw;
            }
            "expense_category" => {
                let raw = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                expense_category = Some(
                    raw.parse::<ExpenseCategory>()
                        .map_err(AppError::BadRequest)?,
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
        .unwrap_or_else(|| mime_guess::mime::APPLICATION_OCTET_STREAM.to_string());
    let name = sanitize_filename(filename.as_deref().unwrap_or("document"));
    let checksum = crate::storage::compute_checksum(&data);
    let now = Utc::now();

    let tags = vec![
        format!("trip:{trip_id}"),
        format!("doctype:{doctype}"),
        "source:driver".into(),
    ];

    let (status_code, record) = if state.store.exists(&checksum).await {
        let existing = state.db.get_one_by_checksum(&checksum).await?;
        let (summary, embedding, status) = match existing {
            Some(ref r) => (r.summary.clone(), r.embedding.clone(), BlobStatus::Ready),
            None => (None, None, BlobStatus::Pending),
        };
        let record = BlobRecord {
            id: Uuid::new_v4(),
            owner_id: 0,
            checksum: checksum.clone(),
            name,
            mime_type,
            size: data.len() as i64,
            status: status.clone(),
            error: None,
            summary,
            tags,
            embedding,
            created_at: now,
            updated_at: now,
            visibility: BlobVisibility::Driver,
            uploaded_by: Some(driver_id),
        };
        state.db.insert(&record).await?;
        if matches!(status, BlobStatus::Pending) {
            let _ = state.pipeline_tx.send(record.id).await;
        }
        (StatusCode::CREATED, record)
    } else {
        let _ = state.store.write(&data).await?;
        let record = BlobRecord {
            id: Uuid::new_v4(),
            owner_id: 0,
            checksum,
            name,
            mime_type,
            size: data.len() as i64,
            status: BlobStatus::Pending,
            error: None,
            summary: None,
            tags,
            embedding: None,
            created_at: now,
            updated_at: now,
            visibility: BlobVisibility::Driver,
            uploaded_by: Some(driver_id),
        };
        state.db.insert(&record).await?;
        let _ = state.pipeline_tx.send(record.id).await;
        (StatusCode::ACCEPTED, record)
    };

    if let Some(load_id) = load_id {
        let load = state.db.get_load_by_id(load_id).await?;
        if !load.blob_ids.contains(&record.id) {
            let mut new_ids = load.blob_ids.clone();
            new_ids.push(record.id);
            state.db.update_load_metadata(
                load_id, None, None, None, None, None, None, None,
                None, None, Some(new_ids), None,
            ).await?;
        }
    }

    if doctype == "expense" {
        let now = Utc::now();
        let expense = crate::models::ExpenseRecord {
            id: Uuid::new_v4(),
            status: crate::models::ExpenseStatus::Submitted,
            category: expense_category.unwrap_or(crate::models::ExpenseCategory::Other),
            driver_id: Some(driver_id),
            trip_id: Some(trip_id),
            equipment_type: None,
            equipment_id: None,
            maintenance_id: None,
            blob_ids: vec![record.id],
            submitted_by: format!("driver:{driver_id}"),
            expense_date: None,
            vendor: None,
            amount: None,
            approved_amount: None,
            payment_method: None,
            suggested_amount: None,
            suggested_date: None,
            suggested_vendor: None,
            suggested_card_last4: None,
            reviewed_by: None,
            reviewed_at: None,
            review_note: None,
            settlement_id: None,
            embedding: None,
            owner_id: 0,
            created_at: now,
            updated_at: now,
        };
        state.db.insert_expense(&expense).await?;
        crate::events::expense_submitted(&state.db, expense.id, Some(expense.submitted_by.clone())).await;
    }

    Ok((status_code, Json(record)))
}

#[utoipa::path(
    get,
    path = "/driver/api/v1/trips/{id}/documents",
    responses(
        (status = 200, description = "List of documents visible to this driver", body = Vec<crate::models::BlobListItem>),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Trip not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "driver"
)]
pub async fn list_documents(
    State(state): State<AppState>,
    Extension(claims): Extension<DriverClaims>,
    Path(trip_id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let driver_id = claims
        .driver_id
        .parse::<Uuid>()
        .map_err(|_| AppError::Unauthorized)?;
    assert_trip_belongs_to_driver(&state, trip_id, driver_id).await?;
    let tag = format!("trip:{trip_id}");
    let (_total, items) = state.db.list(None, &[tag], false, 200, 0).await?;
    let visible: Vec<_> = items
        .into_iter()
        .filter(|b| b.visibility == BlobVisibility::Driver || b.uploaded_by == Some(driver_id))
        .collect();
    Ok(Json(visible))
}

#[utoipa::path(
    get,
    path = "/driver/api/v1/trips/{id}/documents/{blob_id}/content",
    responses(
        (status = 200, description = "Document bytes"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Trip or document not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "driver"
)]
pub async fn get_document_content(
    State(state): State<AppState>,
    Extension(claims): Extension<DriverClaims>,
    Path((trip_id, blob_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, AppError> {
    let driver_id = claims
        .driver_id
        .parse::<Uuid>()
        .map_err(|_| AppError::Unauthorized)?;
    assert_trip_belongs_to_driver(&state, trip_id, driver_id).await?;
    let record = state.db.get_by_id(blob_id).await?;
    let trip_tag = format!("trip:{trip_id}");
    let tagged = record.tags.iter().any(|t| t == &trip_tag);
    let visible =
        record.visibility == BlobVisibility::Driver || record.uploaded_by == Some(driver_id);
    if !tagged || !visible {
        return Err(AppError::NotFound);
    }
    let bytes = state.store.read(&record.checksum).await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        record
            .mime_type
            .parse()
            .unwrap_or(header::HeaderValue::from_static("application/octet-stream")),
    );
    let safe_name = sanitize_filename(&record.name);
    let disposition = format!("inline; filename=\"{safe_name}\"");
    let disposition_value = disposition
        .parse()
        .unwrap_or(header::HeaderValue::from_static("inline"));
    headers.insert(header::CONTENT_DISPOSITION, disposition_value);
    Ok((StatusCode::OK, headers, Body::from(bytes)))
}

#[utoipa::path(
    delete,
    path = "/driver/api/v1/trips/{id}/documents/{blob_id}",
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden: only the uploader can delete"),
        (status = 404, description = "Not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "driver"
)]
pub async fn delete_document(
    State(state): State<AppState>,
    Extension(claims): Extension<DriverClaims>,
    Path((trip_id, blob_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, AppError> {
    let driver_id = claims
        .driver_id
        .parse::<Uuid>()
        .map_err(|_| AppError::Unauthorized)?;
    assert_trip_belongs_to_driver(&state, trip_id, driver_id).await?;
    let record = state.db.get_by_id(blob_id).await?;
    let trip_tag = format!("trip:{trip_id}");
    if !record.tags.iter().any(|t| t == &trip_tag) {
        return Err(AppError::NotFound);
    }
    if record.uploaded_by != Some(driver_id) {
        return Err(AppError::Forbidden("not the uploader of this document".into()));
    }
    state.db.delete_by_id(blob_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
