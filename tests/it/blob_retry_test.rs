// #406 — end-to-end coverage of which failures spend a blob's retry budget.
//
// The unit tests in src/db/blob_ops.rs drive mark_failed with an explicit
// BlobFailureKind, so they pin the *mechanism* and nothing about the wiring:
// swapping the two arguments in the pipeline would leave them green. These
// tests go through process_blob so the classification itself is under test.
use crate::common::{
    ai_client, mock_ollama, mock_ollama_rejecting, mock_ollama_rejecting_on,
    mock_ollama_wrong_embed_dim, seed_blob, TEST_EMBED_DIM,
};
use ollie::{
    models::{BlobStatus, MAX_PROCESSING_ATTEMPTS},
    pipeline::{worker::process_blob, PipelineJob},
};
use std::sync::Arc;

/// An unreachable Ollama is the outage case. However many times it happens, it
/// must never cost the document its budget — this is the #406 criterion that a
/// still-down dependency cannot permanently write off a fine document.
#[tokio::test]
async fn dependency_outage_never_spends_the_retry_budget() {
    let (id, db, store, _db_dir, _blob_dir, extract_dir) =
        seed_blob(b"a readable text document".to_vec(), "text/plain").await;
    // Nothing listens here, so every Ollama call is a transport error.
    let ai = ai_client("http://127.0.0.1:1");

    for pass in 1..=(MAX_PROCESSING_ATTEMPTS + 2) {
        process_blob(id, &db, &store, &ai, extract_dir.path().to_str().unwrap())
            .await
            .unwrap();
        let record = db.get_by_id(id).await.unwrap();
        assert_eq!(
            record.processing_attempts, 0,
            "outage pass {pass} must not spend the budget"
        );
        assert_eq!(
            record.status,
            BlobStatus::Failed,
            "outage pass {pass} must leave the blob retryable"
        );
    }

    // Still visible to startup recovery after more failures than the cap.
    assert!(db.list_non_ready_ids().await.unwrap().contains(&id));
}

/// A reachable model that *rejects the input* (413) has seen these bytes and
/// refused them — the context-overflow case. That repeats identically on every
/// retry, so it spends the budget and eventually stops being requeued.
#[tokio::test]
async fn a_rejecting_model_spends_the_budget_and_stops_being_requeued() {
    let (id, db, store, _db_dir, _blob_dir, extract_dir) =
        seed_blob(b"a readable text document".to_vec(), "text/plain").await;
    let ai = ai_client(&mock_ollama_rejecting(413).await);

    for expected in 1..MAX_PROCESSING_ATTEMPTS {
        process_blob(id, &db, &store, &ai, extract_dir.path().to_str().unwrap())
            .await
            .unwrap();
        let record = db.get_by_id(id).await.unwrap();
        assert_eq!(record.processing_attempts, expected);
        assert_eq!(record.status, BlobStatus::Failed);
        assert!(db.list_non_ready_ids().await.unwrap().contains(&id));
    }

    process_blob(id, &db, &store, &ai, extract_dir.path().to_str().unwrap())
        .await
        .unwrap();
    let record = db.get_by_id(id).await.unwrap();
    assert_eq!(record.processing_attempts, MAX_PROCESSING_ATTEMPTS);
    assert_eq!(record.status, BlobStatus::PermanentlyFailed);
    assert!(
        !db.list_non_ready_ids().await.unwrap().contains(&id),
        "a written-off blob must drop out of startup recovery"
    );

    // resummarize_blob's path is the way back, with a full budget.
    db.mark_pending(id).await.unwrap();
    let record = db.get_by_id(id).await.unwrap();
    assert_eq!(record.status, BlobStatus::Pending);
    assert_eq!(record.processing_attempts, 0);
    assert!(db.list_non_ready_ids().await.unwrap().contains(&id));
}

/// A reachable Ollama that answers 5xx or 404 is *not* evidence about the
/// document: 404 is a model that was never pulled, 500 is usually a model load
/// failure or OOM, and 502/503/504 is a proxy over an Ollama that is down. Each
/// hits every blob in the batch alike, so spending the budget here would walk
/// the whole backlog to permanently_failed in three restarts — the exact harm
/// #406 exists to prevent (a bad deploy, not a bad document).
#[tokio::test]
async fn a_service_fault_status_never_spends_the_retry_budget() {
    for code in [404u16, 500, 503] {
        let (id, db, store, _db_dir, _blob_dir, extract_dir) =
            seed_blob(b"a readable text document".to_vec(), "text/plain").await;
        let ai = ai_client(&mock_ollama_rejecting(code).await);

        for _ in 0..=MAX_PROCESSING_ATTEMPTS {
            process_blob(id, &db, &store, &ai, extract_dir.path().to_str().unwrap())
                .await
                .unwrap();
        }
        let record = db.get_by_id(id).await.unwrap();
        assert_eq!(record.processing_attempts, 0, "{code} must not spend the budget");
        assert_eq!(record.status, BlobStatus::Failed, "{code} must stay retryable");
        assert!(db.list_non_ready_ids().await.unwrap().contains(&id));
    }
}

/// The embed leg of the classification. The test above never reaches it —
/// summarization fails first — so a regression in `embed`'s error construction
/// would go unnoticed. Here generate succeeds and only /api/embeddings rejects.
#[tokio::test]
async fn the_embed_call_is_classified_too() {
    // 413 on embed: the model refused this text — document-scoped.
    let (id, db, store, _db_dir, _blob_dir, extract_dir) =
        seed_blob(b"a readable text document".to_vec(), "text/plain").await;
    let ai = ai_client(&mock_ollama_rejecting_on(413, false, true).await);
    process_blob(id, &db, &store, &ai, extract_dir.path().to_str().unwrap())
        .await
        .unwrap();
    let record = db.get_by_id(id).await.unwrap();
    assert_eq!(record.processing_attempts, 1, "a 413 from embed spends the budget");
    assert_eq!(record.status, BlobStatus::Failed);

    // 503 on embed: the service is down — must stay free.
    let (id2, db2, store2, _d2, _b2, extract2) =
        seed_blob(b"a readable text document".to_vec(), "text/plain").await;
    let ai2 = ai_client(&mock_ollama_rejecting_on(503, false, true).await);
    process_blob(id2, &db2, &store2, &ai2, extract2.path().to_str().unwrap())
        .await
        .unwrap();
    let record2 = db2.get_by_id(id2).await.unwrap();
    assert_eq!(record2.processing_attempts, 0, "a 503 from embed must not spend the budget");
    assert_eq!(record2.status, BlobStatus::Failed);
}

/// A document whose multibyte characters straddle the summarize (4000) and
/// embed (8000) byte caps must process normally. A naive `&text[..4000]` panics
/// mid-character — and since #406 re-queues failed blobs, that panic would
/// recur on every restart instead of failing the blob once, uncapped because
/// `run_job` correctly refuses to charge an unattributed panic to the document.
#[tokio::test]
async fn a_document_with_multibyte_chars_on_the_truncation_boundary_processes() {
    // 'é' is 2 bytes: an odd-length ASCII prefix puts a char across both caps.
    let mut text = "a".repeat(3999);
    text.push_str(&"é".repeat(3000));
    assert!(!text.is_char_boundary(4000) || !text.is_char_boundary(8000));

    let (id, db, store, _db_dir, _blob_dir, extract_dir) =
        seed_blob(text.clone().into_bytes(), "text/plain").await;
    // An empty summary is what forces the *embed* cap to be exercised too:
    // `embeddable_source` then falls back to the full extracted text, so
    // summarize sees the 4000 boundary and embed sees the 8000 one. With a
    // non-empty summary, embed would only ever see that short string.
    let ai = ai_client(&mock_ollama("").await);

    process_blob(id, &db, &store, &ai, extract_dir.path().to_str().unwrap())
        .await
        .unwrap();

    let record = db.get_by_id(id).await.unwrap();
    assert_eq!(record.status, BlobStatus::Ready);
    assert!(record.embedding.is_some(), "the fallback text must have embedded");
    assert_eq!(record.processing_attempts, 0);

    // And the ordinary path, where a real summary comes back.
    let (id2, db2, store2, _d2, _b2, extract2) =
        seed_blob(text.into_bytes(), "text/plain").await;
    let ai2 = ai_client(&mock_ollama("a summary").await);
    process_blob(id2, &db2, &store2, &ai2, extract2.path().to_str().unwrap())
        .await
        .unwrap();
    let record2 = db2.get_by_id(id2).await.unwrap();
    assert_eq!(record2.status, BlobStatus::Ready);
    assert_eq!(record2.summary.as_deref(), Some("a summary"));
}

/// The panic path. An embed-model swap without an `OLLAMA_EMBED_DIM` update
/// makes Ollama return a vector of the wrong length, which trips an arrow
/// length assert inside `mark_ready` — a *config* fault that arrives as an
/// unattributed panic. `run_job` catches it and ends the pass; that must not
/// spend the document's budget, or three restarts during one bad deploy write
/// off every queued document (#406).
///
/// This goes through `spawn_pipeline` rather than calling `process_blob`
/// directly, because the classification under test lives in `run_job`'s panic
/// handler — a direct call would just propagate the panic and prove nothing.
#[tokio::test]
async fn an_unattributed_panic_does_not_spend_the_retry_budget() {
    let (id, db, store, _db_dir, _blob_dir, extract_dir) =
        seed_blob(b"a readable text document".to_vec(), "text/plain").await;
    // The DB is built for TEST_EMBED_DIM; hand the pipeline a longer vector.
    let ai = ai_client(&mock_ollama_wrong_embed_dim(TEST_EMBED_DIM * 2).await);

    let db = Arc::new(db);
    let tx = ollie::pipeline::spawn_pipeline(
        1,
        db.clone(),
        Arc::new(store),
        Arc::new(ai),
        extract_dir.path().to_str().unwrap().to_string(),
    );

    for pass in 1..=(MAX_PROCESSING_ATTEMPTS + 1) {
        tx.send(PipelineJob::Process(id)).await.unwrap();
        // The worker survives the panic and settles the blob out of Processing.
        let record = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let r = db.get_by_id(id).await.unwrap();
                if r.status != BlobStatus::Processing && r.status != BlobStatus::Pending {
                    return r;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the pipeline worker must survive the panic and end the pass");

        assert_eq!(
            record.processing_attempts, 0,
            "panic pass {pass} must not spend the budget — the bytes were never at fault"
        );
        assert_eq!(record.status, BlobStatus::Failed, "panic pass {pass}");
        db.mark_processing(id).await.unwrap();
    }

    db.mark_failed(
        id,
        "settle".into(),
        ollie::models::BlobFailureKind::Dependency,
    )
    .await
    .unwrap();
    assert!(
        db.list_non_ready_ids().await.unwrap().contains(&id),
        "a blob only ever hit by config faults must stay retryable"
    );
}
