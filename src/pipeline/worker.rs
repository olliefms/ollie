// src/pipeline/worker.rs
use crate::{
    ai::{
        embed::embed_text,
        extract::{extract_content, fit_image_for_vision, scanned_pdf_page_image, Extractable},
        summarize::{describe_image, summarize_text},
        OllamaClient,
    },
    db::DbClient,
    error::AppError,
    storage::{extract_store::write_extract, BlobStore},
};
use chrono::SecondsFormat;
use uuid::Uuid;

fn now_z() -> String {
    chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Maximum binary payload sent to the Ollama vision model. Anything larger
/// overflows moondream's ~8K-token context window and Ollama returns an
/// opaque 500. ~500 KB binary ≈ 670 KB base64.
const MAX_VISION_BYTES: usize = 500_000;

pub async fn process_blob(
    id: Uuid,
    db: &DbClient,
    store: &BlobStore,
    ai: &OllamaClient,
    extract_base: &str,
) -> Result<(), AppError> {
    db.mark_processing(id).await?;
    if let Err(e) = db.append_event("blob", id, "processing_started", None, Some("pipeline"), &now_z(), Some(ai)).await {
        tracing::warn!("event append failed for {id} (processing_started): {e}");
    }

    let record = db.get_by_id(id).await?;
    let data = store.read(&record.checksum).await?;
    let extractable = extract_content(&data, &record.mime_type);

    if let Err(e) = write_extract(extract_base, &record.checksum, &extractable).await {
        tracing::warn!("failed to write extract cache for {id}: {e}");
    }

    let result: Result<(Option<String>, Option<Vec<f32>>), AppError> = async {
        match extractable {
            Extractable::Text(text) => {
                if text.trim().is_empty() {
                    tracing::info!("extracted text for {id} is empty; skipping summarization");
                    Ok((None, None))
                } else {
                    let summary = summarize_text(ai, &text).await?;
                    let embed_source = if summary.is_empty() { &text } else { &summary };
                    let embedding = embed_text(ai, embed_source).await?;
                    Ok((Some(summary), Some(embedding)))
                }
            }
            Extractable::ScannedPdf(bytes, raw_text) => {
                // A scanned PDF's page is usually one full-page JPEG; recover it
                // and describe it with the vision model. Raw PDF bytes must never
                // reach the vision model — they crash Ollama's CLIP tokenizer
                // (SIGSEGV, #281) — so only a validated, size-fitted page image is
                // sent. When no usable image is found, degrade to whatever text
                // the extractor produced, as before. (#367)
                let page_image = tokio::task::spawn_blocking(move || {
                    scanned_pdf_page_image(&bytes)
                        .and_then(|img| fit_image_for_vision(&img, MAX_VISION_BYTES))
                })
                .await
                .ok()
                .flatten();
                match page_image {
                    Some(img) => {
                        let description = describe_image(ai, &bytes::Bytes::from(img)).await?;
                        let embedding = embed_text(ai, &description).await?;
                        Ok((Some(description), Some(embedding)))
                    }
                    None if raw_text.trim().is_empty() => {
                        tracing::info!("scanned PDF {id} has no extractable text or usable page image; skipping summarization");
                        Ok((None, None))
                    }
                    None => {
                        let summary = summarize_text(ai, &raw_text).await?;
                        let embed_source = if summary.is_empty() { &raw_text } else { &summary };
                        let embedding = embed_text(ai, embed_source).await?;
                        Ok((Some(summary), Some(embedding)))
                    }
                }
            }
            Extractable::ImageBytes(bytes) => {
                // Oversized images are downscaled to fit the vision budget instead
                // of being skipped outright; only undecodable/unshrinkable payloads
                // still skip AI description. (#367)
                let fitted = tokio::task::spawn_blocking(move || {
                    fit_image_for_vision(&bytes, MAX_VISION_BYTES)
                })
                .await
                .ok()
                .flatten();
                match fitted {
                    Some(img) => {
                        let description = describe_image(ai, &bytes::Bytes::from(img)).await?;
                        let embedding = embed_text(ai, &description).await?;
                        Ok((Some(description), Some(embedding)))
                    }
                    None => {
                        tracing::info!("image {id} could not be fitted for the vision model; skipping AI description");
                        Ok((None, None))
                    }
                }
            }
            Extractable::Unsupported => Ok((None, None)),
        }
    }.await;

    match result {
        Ok((summary, embedding)) => {
            db.mark_ready(id, summary, embedding).await?;
            if let Err(e) = db.append_event("blob", id, "processing_completed", None, Some("pipeline"), &now_z(), Some(ai)).await {
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
                Some("pipeline"), &now_z(), Some(ai),
            ).await {
                tracing::warn!("event append failed for {id} (processing_failed): {ev_err}");
            }
        }
    }

    Ok(())
}
