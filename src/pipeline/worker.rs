// src/pipeline/worker.rs
use crate::{
    ai::{
        embed::embed_text,
        extract::{extract_content, Extractable},
        summarize::{describe_image, describe_scanned_pdf, summarize_text},
        OllamaClient,
    },
    db::DbClient,
    error::AppError,
    storage::BlobStore,
};
use chrono::SecondsFormat;
use uuid::Uuid;

fn now_z() -> String {
    chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub async fn process_blob(
    id: Uuid,
    db: &DbClient,
    store: &BlobStore,
    ai: &OllamaClient,
) -> Result<(), AppError> {
    db.mark_processing(id).await?;
    if let Err(e) = db.append_event("blob", id, "processing_started", None, Some("pipeline"), &now_z()).await {
        tracing::warn!("event append failed for {id} (processing_started): {e}");
    }

    let record = db.get_by_id(id).await?;
    let data = store.read(&record.checksum).await?;
    let extractable = extract_content(&data, &record.mime_type);

    let result: Result<(Option<String>, Option<Vec<f32>>), AppError> = async {
        match extractable {
            Extractable::Text(text) => {
                let summary = summarize_text(ai, &text).await?;
                let embed_source = if summary.is_empty() { &text } else { &summary };
                let embedding = embed_text(ai, embed_source).await?;
                Ok((Some(summary), Some(embedding)))
            }
            Extractable::ScannedPdf(bytes, raw_text) => {
                let description = describe_scanned_pdf(ai, &bytes, &raw_text).await?;
                let embedding = embed_text(ai, &description).await?;
                Ok((Some(description), Some(embedding)))
            }
            Extractable::ImageBytes(bytes) => {
                let description = describe_image(ai, &bytes).await?;
                let embedding = embed_text(ai, &description).await?;
                Ok((Some(description), Some(embedding)))
            }
            Extractable::Unsupported => Ok((None, None)),
        }
    }.await;

    match result {
        Ok((summary, embedding)) => {
            db.mark_ready(id, summary, embedding).await?;
            if let Err(e) = db.append_event("blob", id, "processing_completed", None, Some("pipeline"), &now_z()).await {
                tracing::warn!("event append failed for {id} (processing_completed): {e}");
            }
            tracing::info!("pipeline completed for {id}");
        }
        Err(e) => {
            tracing::error!("pipeline failed for {id}: {e}");
            let err_str = e.to_string();
            db.mark_failed(id, err_str.clone()).await?;
            if let Err(ev_err) = db.append_event(
                "blob", id, "processing_failed",
                Some(serde_json::json!({ "error": err_str })),
                Some("pipeline"), &now_z(),
            ).await {
                tracing::warn!("event append failed for {id} (processing_failed): {ev_err}");
            }
        }
    }

    Ok(())
}
