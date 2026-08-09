// tests/isolated/pipeline_ocr_test.rs
//
// #372: scanned PDFs are summarized OCR-first — when tesseract recovers real
// text from the page image, the text model summarizes it and the vision model
// is never needed. The tesseract binary is faked via OLLIE_TESSERACT_BIN
// (process-global env — serialised against the other pipeline tests via
// common::ENV_LOCK, held across the process_blob await).
use crate::common;

use ollie::models::blob::BlobStatus;
use ollie::pipeline::worker::process_blob;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;

/// Write an executable fake-tesseract script that drains stdin and prints
/// fixed OCR text.
fn fake_tesseract(dir: &std::path::Path, ocr_text: &str) -> String {
    let path = dir.join("fake-tesseract");
    let mut f = std::fs::File::create(&path).unwrap();
    write!(f, "#!/bin/sh\ncat >/dev/null\ncat <<'EOT'\n{ocr_text}\nEOT\n").unwrap();
    drop(f);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.to_str().unwrap().to_string()
}

const OCR_TEXT: &str = "BOSS SHOP RAPID CITY invoice 110148792 dated 04/28/26 \
truck 581400 replace air filter bolt stripped had to cut off parts run labor \
105.00 parts 189.99 shop supply 35.40 tax 20.48 total 350.87 fleet paid";

// ENV_LOCK is deliberately held across the process_blob await: the env var it
// guards is read inside that call, and each #[tokio::test] blocks only its own
// thread's current-thread runtime.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn test_ocr_text_is_summarized_by_text_model_and_blob_ready() {
    let _env = common::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let script_dir = tempfile::TempDir::new().unwrap();
    std::env::set_var("OLLIE_TESSERACT_BIN", fake_tesseract(script_dir.path(), OCR_TEXT));

    let base_url = common::mock_ollama("Boss Shop invoice for an air filter replacement.").await;
    let ai = common::ai_client(&base_url);
    let (id, db, store, _d1, _d2, extract_dir) =
        common::seed_blob(common::scanned_pdf(), "application/pdf").await;

    process_blob(id, &db, &store, &ai, extract_dir.path().to_str().unwrap())
        .await
        .unwrap();

    let record = db.get_by_id(id).await.unwrap();
    assert_eq!(record.status, BlobStatus::Ready, "error={:?}", record.error);
    assert_eq!(
        record.summary.as_deref(),
        Some("Boss Shop invoice for an air filter replacement."),
        "OCR text must flow through the text-model summary"
    );
    assert!(record.embedding.is_some(), "summary must be embedded");
}
