// src/pipeline/worker.rs
use crate::{
    ai::{
        embed::embed_text,
        extract::{extract_content, Extractable},
        summarize::{describe_image, summarize_text},
        OllamaClient,
    },
    db::DbClient,
    error::AppError,
    storage::BlobStore,
};
use uuid::Uuid;

pub async fn process_blob(
    id: Uuid,
    db: &DbClient,
    store: &BlobStore,
    ai: &OllamaClient,
) -> Result<(), AppError> {
    db.mark_processing(id).await?;

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
            Extractable::ImageBytes(bytes) => {
                let description = describe_image(ai, &bytes).await?;
                let embedding = embed_text(ai, &description).await?;
                Ok((Some(description), Some(embedding)))
            }
            // TODO: replace with hybrid vision+text extraction once issue #3 is resolved
            Extractable::GibberishPdf => Err(AppError::Internal(
                "extraction_failed: PDF text is gibberish (CID/custom font encoding); \
                 hybrid vision extraction pending issue #3"
                    .into(),
            )),
            Extractable::Unsupported => Ok((None, None)),
        }
    }.await;

    match result {
        Ok((summary, embedding)) => {
            db.mark_ready(id, summary, embedding).await?;
            tracing::info!("pipeline completed for {id}");
        }
        Err(e) => {
            tracing::error!("pipeline failed for {id}: {e}");
            db.mark_failed(id, e.to_string()).await?;
        }
    }

    Ok(())
}
