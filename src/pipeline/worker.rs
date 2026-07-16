// src/pipeline/worker.rs
use crate::{
    ai::{
        embed::embed_text,
        extract::{extract_content, fit_image_for_vision, scanned_pdf_page_image, word_count, Extractable},
        ocr::ocr_image,
        summarize::{describe_image, summarize_document_text, summarize_text},
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

/// Maximum pixel long edge sent to the vision model. moondream returns an
/// EMPTY response for full-resolution scans (e.g. 2432×3168) even when the
/// byte payload is under MAX_VISION_BYTES — the trigger is resolution, not
/// bytes. (#372)
const MAX_VISION_DIM: u32 = 1024;

/// Minimum OCR word count to trust tesseract output over the vision model.
/// Below this the "text" is usually noise from a photo, not a document.
const OCR_MIN_WORDS: usize = 20;

/// What produced the stored summary — logged and recorded on the
/// processing_completed event so OCR-needed docs can be told apart from
/// genuinely empty ones. (#372)
#[derive(Clone, Copy, PartialEq, Debug)]
enum SummarySource {
    Text,
    Ocr,
    Vision,
    PdfText,
    Preserved,
    None,
}

impl SummarySource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Ocr => "ocr",
            Self::Vision => "vision",
            Self::PdfText => "pdf_text",
            Self::Preserved => "preserved",
            Self::None => "none",
        }
    }
}

/// Pick what to embed: the summary when it has content, else the source text
/// it was derived from. None when both are empty — empty text must never
/// reach embed_text (it errors, and that error used to flip ready blobs to
/// failed, #372).
fn embeddable_source<'a>(summary: &'a str, fallback: &'a str) -> Option<&'a str> {
    if !summary.trim().is_empty() {
        Some(summary)
    } else if !fallback.trim().is_empty() {
        Some(fallback)
    } else {
        None
    }
}

/// Summarize a document page / image payload, OCR-first.
///
/// Local vision models cannot reliably read document scans (moondream returns
/// empty or hallucinated text for them), but tesseract reads printed scans
/// well and the text model summarizes OCR text accurately. So: OCR at full
/// resolution first; when it yields enough words, summarize that. The vision
/// model is the fallback for image content with no machine-readable text
/// (freight photos, handwriting). Returns (summary, embed-fallback text,
/// source), or None when neither path produced anything. (#372)
async fn summarize_image_payload(
    ai: &OllamaClient,
    image: Vec<u8>,
) -> Result<Option<(String, String, SummarySource)>, AppError> {
    if let Some(text) = ocr_image(&image).await {
        if word_count(&text) >= OCR_MIN_WORDS {
            let summary = summarize_document_text(ai, &text).await?;
            return Ok(Some((summary, text, SummarySource::Ocr)));
        }
    }
    let fitted = tokio::task::spawn_blocking(move || {
        fit_image_for_vision(&image, MAX_VISION_BYTES, MAX_VISION_DIM)
    })
    .await
    .ok()
    .flatten();
    match fitted {
        Some(img) => {
            let description = describe_image(ai, &bytes::Bytes::from(img)).await?;
            if description.trim().is_empty() {
                Ok(None)
            } else {
                let fallback = description.clone();
                Ok(Some((description, fallback, SummarySource::Vision)))
            }
        }
        None => Ok(None),
    }
}

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

    let result: Result<(Option<String>, String, SummarySource), AppError> = async {
        match extractable {
            Extractable::Text(text) => {
                if text.trim().is_empty() {
                    tracing::info!("extracted text for {id} is empty; skipping summarization");
                    Ok((None, String::new(), SummarySource::None))
                } else {
                    let summary = summarize_text(ai, &text).await?;
                    Ok((Some(summary), text, SummarySource::Text))
                }
            }
            Extractable::ScannedPdf(bytes, raw_text) => {
                // Raw PDF bytes must never reach the vision model — they crash
                // Ollama's CLIP tokenizer (SIGSEGV, #281). Only the recovered,
                // validated page image is used, OCR-first (#372); when nothing
                // is recoverable, degrade to whatever text the extractor got.
                let page_image = tokio::task::spawn_blocking(move || scanned_pdf_page_image(&bytes))
                    .await
                    .ok()
                    .flatten();
                let outcome = match page_image {
                    Some(img) => summarize_image_payload(ai, img).await?,
                    None => None,
                };
                match outcome {
                    Some((summary, fallback, source)) => Ok((Some(summary), fallback, source)),
                    None if raw_text.trim().is_empty() => {
                        tracing::info!("scanned PDF {id} has no OCR text, usable page image, or text layer; skipping summarization");
                        Ok((None, String::new(), SummarySource::None))
                    }
                    None => {
                        let summary = summarize_text(ai, &raw_text).await?;
                        Ok((Some(summary), raw_text, SummarySource::PdfText))
                    }
                }
            }
            Extractable::ImageBytes(bytes) => {
                // Phone photos of documents (PODs, receipts) go OCR-first too;
                // scenic photos yield no OCR text and fall through to vision.
                match summarize_image_payload(ai, bytes.to_vec()).await? {
                    Some((summary, fallback, source)) => Ok((Some(summary), fallback, source)),
                    None => {
                        tracing::info!("image {id} produced no OCR text or vision description; skipping AI description");
                        Ok((None, String::new(), SummarySource::None))
                    }
                }
            }
            Extractable::Unsupported => Ok((None, String::new(), SummarySource::None)),
        }
    }.await;

    // Resolve the embedding AFTER the extraction/summarization result so the
    // empty-guard applies uniformly: nothing embeddable → ready with no
    // summary, never a failure. (#372)
    let result = match result {
        Ok((summary, fallback, source)) => {
            match embeddable_source(summary.as_deref().unwrap_or(""), &fallback) {
                Some(src) => embed_text(ai, src)
                    .await
                    .map(|embedding| (summary, Some(embedding), source)),
                // Nothing readable this run. If the blob already carries a
                // summary (e.g. manually backfilled via update_blob), keep it —
                // a reprocess must never degrade an already-good blob. (#372)
                None if record.summary.as_deref().is_some_and(|s| !s.trim().is_empty()) => {
                    tracing::info!("no content recovered for {id}; preserving existing summary");
                    Ok((record.summary.clone(), record.embedding.clone(), SummarySource::Preserved))
                }
                None => Ok((None, None, SummarySource::None)),
            }
        }
        Err(e) => Err(e),
    };

    match result {
        Ok((summary, embedding, source)) => {
            db.mark_ready(id, summary, embedding).await?;
            if let Err(e) = db.append_event(
                "blob", id, "processing_completed",
                Some(serde_json::json!({ "summary_source": source.as_str() })),
                Some("pipeline"), &now_z(), Some(ai),
            ).await {
                tracing::warn!("event append failed for {id} (processing_completed): {e}");
            }
            tracing::info!("pipeline completed for {id} (summary source: {})", source.as_str());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embeddable_source_prefers_summary() {
        assert_eq!(embeddable_source("a summary", "fallback"), Some("a summary"));
    }

    #[test]
    fn test_embeddable_source_falls_back_on_whitespace_summary() {
        assert_eq!(embeddable_source(" \n\t ", "fallback text"), Some("fallback text"));
        assert_eq!(embeddable_source("", "fallback text"), Some("fallback text"));
    }

    #[test]
    fn test_embeddable_source_none_when_both_empty() {
        assert_eq!(embeddable_source("", ""), None);
        assert_eq!(embeddable_source(" \n ", "\t"), None);
    }
}
