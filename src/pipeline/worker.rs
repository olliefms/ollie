// src/pipeline/worker.rs
use crate::{
    ai::{
        embed::embed_text,
        expense_fields::extract_expense_fields,
        extract::{
            extract_content, fit_image_for_vision, scanned_pdf_page_image, word_count,
            Extractable, MAX_VISION_BYTES, MAX_VISION_DIM,
        },
        ocr::ocr_image,
        summarize::{describe_image, summarize_document_text, summarize_text},
        OllamaClient,
    },
    db::DbClient,
    error::AppError,
    models::ExpenseStatus,
    storage::{extract_store::write_extract, BlobStore},
};
use chrono::SecondsFormat;
use uuid::Uuid;

fn now_z() -> String {
    chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

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

/// Run content extraction on the blocking pool.
///
/// `extract_content` is sync CPU work, and for PDFs it hands the bytes to
/// `pdf-extract`, which panics (rather than erroring) on malformed input. On the
/// worker task that panic kills the worker; on the blocking pool it comes back
/// as a `JoinError` this can report per blob. `None` means "the extractor died
/// on these bytes" — the caller decides what that costs.
async fn extract_off_task(data: bytes::Bytes, mime_type: String) -> Option<Extractable> {
    tokio::task::spawn_blocking(move || extract_content(&data, &mime_type)).await.ok()
}

/// End a failed `Process` pass without degrading a blob that is already good.
///
/// `mark_failed` leaves `summary`/`embedding` in place but moves the blob out of
/// `ready`, and `db.search` filters on `status = 'ready'` — so failing a blob
/// that already carries a summary (a manual `update_blob` backfill, or an
/// earlier successful run) silently drops it out of semantic search. A reprocess
/// must never degrade an already-good blob (#372), so this preserves it exactly
/// as the "nothing recovered" branch of `process_blob` does, and only marks
/// failed when there is nothing worth keeping.
pub(crate) async fn fail_without_degrading(
    id: Uuid,
    db: &DbClient,
    ai: &OllamaClient,
    error: String,
) -> Result<(), AppError> {
    let record = db.get_by_id(id).await?;
    if record.summary.as_deref().is_some_and(|s| !s.trim().is_empty()) {
        tracing::error!("{error} for {id}; preserving existing summary");
        db.mark_ready(id, record.summary.clone(), record.embedding.clone()).await?;
        if let Err(e) = db.append_event(
            "blob", id, "processing_completed",
            Some(serde_json::json!({
                "summary_source": SummarySource::Preserved.as_str(),
                "error": error,
            })),
            Some("pipeline"), &now_z(), Some(ai),
        ).await {
            tracing::warn!("event append failed for {id} (processing_completed): {e}");
        }
        return Ok(());
    }
    tracing::error!("pipeline failed for {id}: {error}");
    db.mark_failed(id, error.clone()).await?;
    if let Err(e) = db.append_event(
        "blob", id, "processing_failed",
        Some(serde_json::json!({ "error": error })),
        Some("pipeline"), &now_z(), Some(ai),
    ).await {
        tracing::warn!("event append failed for {id} (processing_failed): {e}");
    }
    Ok(())
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

/// Stage structured receipt suggestions on any *submitted* expense that
/// references this blob. Best-effort — any failure leaves the expense's
/// suggested_* fields alone and never affects blob status.
async fn apply_expense_suggestions(
    id: Uuid,
    db: &DbClient,
    ai: &OllamaClient,
    content: &Extractable,
) {
    if let Some(fields) = extract_expense_fields(ai, content).await {
        if let Ok(expenses) = db.expenses_referencing_blob(id).await {
            for e in expenses {
                if matches!(e.status, ExpenseStatus::Submitted) {
                    let _ = db.update_expense_suggestions(
                        e.id,
                        fields.amount,
                        fields.date.clone(),
                        fields.vendor.clone(),
                        fields.card_last4.clone(),
                    ).await;
                }
            }
        }
    }
}

/// Suggestions-only pass for an already-Ready blob (`PipelineJob::ExpenseSuggestions`):
/// a dedup re-upload of an expense receipt copies the summary/embedding from
/// the existing record at upload time, so the full pass would only waste model
/// calls — but the new expense row still needs its receipt fields extracted.
pub async fn stage_expense_suggestions(
    id: Uuid,
    db: &DbClient,
    store: &BlobStore,
    ai: &OllamaClient,
) -> Result<(), AppError> {
    let record = db.get_by_id(id).await?;
    let data = store.read(&record.checksum).await?;
    // Best-effort pass over a blob that is already Ready: an extractor panic
    // costs the suggestions, nothing else, so it must not touch blob status.
    let Some(extractable) = extract_off_task(data, record.mime_type.clone()).await else {
        tracing::error!("content extraction panicked for {id}; no expense suggestions staged");
        return Ok(());
    };
    apply_expense_suggestions(id, db, ai, &extractable).await;
    Ok(())
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
    let Some(extractable) = extract_off_task(data, record.mime_type.clone()).await else {
        // Extractor panicked on these bytes. That is a failure of this blob, not
        // of the pipeline — end the pass so it leaves Processing and can be
        // re-queued once the input (or the extractor) changes.
        return fail_without_degrading(
            id, db, ai, "content extraction panicked on these bytes".to_string(),
        ).await;
    };

    if let Err(e) = write_extract(extract_base, &record.checksum, &extractable).await {
        tracing::warn!("failed to write extract cache for {id}: {e}");
    }

    // Kept aside (cloned) so the expense-suggestion hook below still has the
    // extracted content after the summary/embedding match below consumes its
    // own copy. Only cloned for blobs tagged as expense receipts.
    let is_expense_doc = record.tags.iter().any(|t| t == "doctype:expense");
    let extractable_for_expense = is_expense_doc.then(|| extractable.clone());

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

            // Expense receipts: stage structured suggestions on any submitted
            // expense that references this blob. Best-effort — extraction
            // failure here is never a processing failure.
            if let Some(content) = &extractable_for_expense {
                apply_expense_suggestions(id, db, ai, content).await;
            }
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
    use crate::models::{BlobRecord, BlobStatus};
    use tempfile::TempDir;

    async fn blob_in_processing(summary: Option<&str>) -> (DbClient, OllamaClient, Uuid, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        let now = chrono::Utc::now();
        let record = BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum: "abc123".into(),
            name: "scan.pdf".into(), mime_type: "application/pdf".into(), size: 42,
            status: BlobStatus::Processing, error: None,
            summary: summary.map(str::to_string),
            tags: vec![], embedding: None, created_at: now, updated_at: now,
            visibility: Default::default(), uploaded_by: None,
        };
        db.insert(&record).await.unwrap();
        // Unreachable Ollama: event embedding is best-effort and must not matter here.
        let ai = OllamaClient::new("http://127.0.0.1:1", "nomic-embed-text", "llama3.2", "moondream");
        (db, ai, record.id, dir)
    }

    /// A pass that dies must not drop a blob that already reads well out of
    /// search — `db.search` filters on `status = 'ready'` (#372).
    #[tokio::test]
    async fn test_failed_pass_preserves_an_existing_summary() {
        let (db, ai, id, _dir) = blob_in_processing(Some("a good summary")).await;
        fail_without_degrading(id, &db, &ai, "extractor panicked".into()).await.unwrap();

        let record = db.get_by_id(id).await.unwrap();
        assert_eq!(record.status, BlobStatus::Ready);
        assert_eq!(record.summary.as_deref(), Some("a good summary"));
    }

    #[tokio::test]
    async fn test_failed_pass_marks_failed_when_there_is_nothing_to_keep() {
        let (db, ai, id, _dir) = blob_in_processing(None).await;
        fail_without_degrading(id, &db, &ai, "extractor panicked".into()).await.unwrap();

        let record = db.get_by_id(id).await.unwrap();
        assert_eq!(record.status, BlobStatus::Failed);
        assert_eq!(record.error.as_deref(), Some("extractor panicked"));
    }

    /// Whitespace is not a summary — it must not rescue a blob from failure.
    #[tokio::test]
    async fn test_failed_pass_ignores_a_whitespace_summary() {
        let (db, ai, id, _dir) = blob_in_processing(Some("   \n\t ")).await;
        fail_without_degrading(id, &db, &ai, "extractor panicked".into()).await.unwrap();

        assert_eq!(db.get_by_id(id).await.unwrap().status, BlobStatus::Failed);
    }

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
