// tests/pipeline_empty_vision_test.rs
//
// Regression test for #372: a scanned (image-only) PDF whose vision
// description comes back EMPTY must leave the blob ready with no summary —
// never mark it failed with "cannot embed empty text". This is exactly what
// production moondream does with full-resolution scans.
//
// Lives in its own test binary: pipeline_ocr_test.rs mutates the
// process-global OLLIE_TESSERACT_BIN env var, and this test must run with
// tesseract genuinely unavailable.
mod common;

use ollie::models::blob::BlobStatus;
use ollie::pipeline::worker::process_blob;

#[tokio::test]
async fn test_empty_vision_description_leaves_blob_ready_without_summary() {
    // Point OCR at a nonexistent binary so the vision path is exercised even
    // on machines that have tesseract installed.
    std::env::set_var("OLLIE_TESSERACT_BIN", "/nonexistent/tesseract-372");

    let base_url = common::mock_ollama("").await; // moondream returns ""
    let ai = common::ai_client(&base_url);
    let (id, db, store, _d1, _d2, extract_dir) =
        common::seed_blob(common::scanned_pdf(), "application/pdf").await;

    process_blob(id, &db, &store, &ai, extract_dir.path().to_str().unwrap())
        .await
        .unwrap();

    let record = db.get_by_id(id).await.unwrap();
    assert_eq!(
        record.status,
        BlobStatus::Ready,
        "blob must stay usable when the vision model returns nothing; error={:?}",
        record.error
    );
    assert!(
        record.summary.as_deref().unwrap_or("").is_empty(),
        "no summary should be stored for an unreadable scan, got {:?}",
        record.summary
    );
    assert!(record.error.is_none(), "no error expected, got {:?}", record.error);
}

#[tokio::test]
async fn test_reprocess_yielding_nothing_preserves_existing_summary() {
    // resummarize_blob on a blob that already has a (possibly manual) summary
    // must not wipe it when the new run can't read the doc — a retry never
    // degrades an already-good blob (#372).
    std::env::set_var("OLLIE_TESSERACT_BIN", "/nonexistent/tesseract-372");

    let base_url = common::mock_ollama("").await; // vision yields nothing
    let ai = common::ai_client(&base_url);
    let (id, db, store, _d1, _d2, extract_dir) =
        common::seed_blob(common::scanned_pdf(), "application/pdf").await;
    db.mark_ready(
        id,
        Some("Manually backfilled summary.".into()),
        Some(vec![0.5f32; common::TEST_EMBED_DIM]),
    )
    .await
    .unwrap();

    process_blob(id, &db, &store, &ai, extract_dir.path().to_str().unwrap())
        .await
        .unwrap();

    let record = db.get_by_id(id).await.unwrap();
    assert_eq!(record.status, BlobStatus::Ready, "error={:?}", record.error);
    assert_eq!(
        record.summary.as_deref(),
        Some("Manually backfilled summary."),
        "existing summary must survive a no-yield reprocess"
    );
    assert!(record.embedding.is_some(), "existing embedding must survive");
}
