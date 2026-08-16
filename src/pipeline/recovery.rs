// src/pipeline/recovery.rs
use crate::{db::DbClient, error::AppError, pipeline::PipelineJob};
use uuid::Uuid;

/// Emit a progress line every this many jobs handed to a pipeline. Past a
/// channel's capacity a send only completes when a worker takes a job, so this
/// rate tracks the drain rate — without it `requeueing N stale blobs on startup`
/// is the last word an operator gets for hours (#404).
const PROGRESS_EVERY: usize = 50;

/// Feed `jobs` into `tx`, logging progress. Every one of these queues is bounded
/// and every one can wedge for hours, so they all report.
async fn drain_into<T: Copy>(
    tx: &async_channel::Sender<T>,
    jobs: &[T],
    label: &str,
) -> Result<(), AppError> {
    let total = jobs.len();
    tracing::info!("requeueing {total} {label} on startup");
    for (i, job) in jobs.iter().enumerate() {
        tx.send(*job).await.map_err(|e| AppError::Internal(e.to_string()))?;
        let done = i + 1;
        if done % PROGRESS_EVERY == 0 || done == total {
            tracing::info!("{label} requeue: {done}/{total} accepted");
        }
    }
    Ok(())
}

/// Re-queue everything a restart left unfinished.
///
/// The three queues drain **concurrently**. Each `drain_into` inherits its
/// channel's drain rate — past `bounded(256)` a send only completes when a
/// worker takes a job — so running them in sequence makes every queue wait out
/// the ones ahead of it. With `PIPELINE_WORKERS` defaulting to 1 at ~40 s a
/// blob, a few hundred stale blobs would hold facility geocoding and load
/// routing for hours, and #406 widened that head of the line by adding retryable
/// failures to it. The queues feed independent worker pools and share no state,
/// so there is nothing to order between them.
///
/// The two *pipeline* drains stay sequential with respect to each other: the
/// suggestions set is derived from the blob sweep's ids, and both feed the same
/// channel.
pub async fn requeue_stale(
    db: &DbClient,
    pipeline_tx: &async_channel::Sender<PipelineJob>,
    geocoding_tx: &async_channel::Sender<Uuid>,
    routing_tx: &async_channel::Sender<Uuid>,
) -> Result<(), AppError> {
    let blobs = async {
        let ids = db.list_non_ready_ids().await?;
        let jobs: Vec<PipelineJob> = ids.iter().map(|id| PipelineJob::Process(*id)).collect();
        drain_into(pipeline_tx, &jobs, "stale blobs").await?;

        // Suggestions-only jobs target already-Ready blobs, so the status sweep
        // above can never recover them. Blobs already queued for a full pass are
        // skipped — that pass stages suggestions itself.
        let queued: std::collections::HashSet<Uuid> = ids.into_iter().collect();
        let needs_suggestions: Vec<PipelineJob> = db.list_blob_ids_needing_expense_suggestions().await?
            .into_iter()
            .filter(|id| !queued.contains(id))
            .map(PipelineJob::ExpenseSuggestions)
            .collect();
        drain_into(pipeline_tx, &needs_suggestions, "blobs for expense suggestions").await
    };

    let geocodes = async {
        let pending_geocode = db.list_pending_geocode_facility_ids().await?;
        drain_into(geocoding_tx, &pending_geocode, "facilities for geocoding").await
    };

    let routes = async {
        let pending_routing = db.list_loads_needing_routing().await?;
        drain_into(routing_tx, &pending_routing, "loads for routing").await
    };

    tokio::try_join!(blobs, geocodes, routes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::DbClient,
        models::{BlobRecord, BlobStatus, ExpenseCategory, ExpenseRecord, ExpenseStatus},
    };
    use chrono::Utc;
    use tempfile::TempDir;
    use uuid::Uuid;

    fn ready_blob(checksum: &str) -> BlobRecord {
        let now = Utc::now();
        BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum: checksum.into(),
            name: "receipt.jpg".into(), mime_type: "image/jpeg".into(), size: 1,
            status: BlobStatus::Ready, error: None, summary: Some("a receipt".into()),
            tags: vec!["doctype:expense".into()], embedding: None,
            created_at: now, updated_at: now,
            visibility: Default::default(), uploaded_by: None,
            processing_attempts: 0,
        }
    }

    fn submitted_expense(blob_id: Uuid) -> ExpenseRecord {
        let now = Utc::now();
        ExpenseRecord {
            id: Uuid::new_v4(),
            status: ExpenseStatus::Submitted,
            category: ExpenseCategory::Fuel,
            driver_id: None, trip_id: None,
            equipment_type: None, equipment_id: None, maintenance_id: None,
            blob_ids: vec![blob_id],
            submitted_by: "driver:test".into(),
            expense_date: None, vendor: None, amount: None,
            approved_amount: None, payment_method: None,
            suggested_amount: None, suggested_date: None,
            suggested_vendor: None, suggested_card_last4: None,
            reviewed_by: None, reviewed_at: None, review_note: None,
            settlement_id: None, embedding: None, owner_id: 0,
            created_at: now, updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_requeue_sends_pending_ids_only() {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        let now = Utc::now();

        let pending = BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum: "c1".into(),
            name: "f.txt".into(), mime_type: "text/plain".into(), size: 1,
            status: BlobStatus::Pending, error: None, summary: None,
            tags: vec![], embedding: None, created_at: now, updated_at: now,
            visibility: Default::default(), uploaded_by: None,
            processing_attempts: 0,
        };
        let ready = BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum: "c2".into(),
            name: "g.txt".into(), mime_type: "text/plain".into(), size: 1,
            status: BlobStatus::Ready, error: None, summary: None,
            tags: vec![], embedding: None, created_at: now, updated_at: now,
            visibility: Default::default(), uploaded_by: None,
            processing_attempts: 0,
        };
        db.insert(&pending).await.unwrap();
        db.insert(&ready).await.unwrap();

        let (tx, rx) = async_channel::bounded(10);
        let (gtx, _) = async_channel::bounded(10);
        let (rtx, _) = async_channel::bounded(10);
        requeue_stale(&db, &tx, &gtx, &rtx).await.unwrap();
        assert_eq!(rx.len(), 1);
        let received = rx.recv().await.unwrap();
        assert_eq!(received, PipelineJob::Process(pending.id));
    }

    /// A `failed` blob is retryable and must come back on the next startup —
    /// that is the whole of #406. A `permanently_failed` one has spent its
    /// document-failure budget and must stay put until `resummarize_blob`.
    #[tokio::test]
    async fn test_requeue_sends_failed_but_not_permanently_failed() {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        let now = Utc::now();

        let failed = BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum: "c-failed".into(),
            name: "f.txt".into(), mime_type: "text/plain".into(), size: 1,
            status: BlobStatus::Failed, error: Some("ollama unreachable".into()),
            summary: None, tags: vec![], embedding: None,
            created_at: now, updated_at: now,
            visibility: Default::default(), uploaded_by: None,
            processing_attempts: 0,
        };
        let permanently_failed = BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum: "c-permanent".into(),
            name: "g.txt".into(), mime_type: "text/plain".into(), size: 1,
            status: BlobStatus::PermanentlyFailed, error: Some("bad bytes".into()),
            summary: None, tags: vec![], embedding: None,
            created_at: now, updated_at: now,
            visibility: Default::default(), uploaded_by: None,
            processing_attempts: 3,
        };
        db.insert(&failed).await.unwrap();
        db.insert(&permanently_failed).await.unwrap();

        let (tx, rx) = async_channel::bounded(10);
        let (gtx, _) = async_channel::bounded(10);
        let (rtx, _) = async_channel::bounded(10);
        requeue_stale(&db, &tx, &gtx, &rtx).await.unwrap();

        assert_eq!(rx.len(), 1, "only the retryable failure should requeue");
        assert_eq!(rx.recv().await.unwrap(), PipelineJob::Process(failed.id));
    }

    /// A suggestions-only job targets an already-Ready blob, so the non-ready
    /// status sweep can never recover it. Startup must re-derive it from the
    /// expense side: a submitted expense with no suggestions gets its receipt
    /// re-queued, while one that already carries suggestions does not.
    #[tokio::test]
    async fn test_requeue_recovers_missing_expense_suggestions() {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();

        let unsuggested_blob = ready_blob("c-unsuggested");
        let suggested_blob = ready_blob("c-suggested");
        db.insert(&unsuggested_blob).await.unwrap();
        db.insert(&suggested_blob).await.unwrap();

        db.insert_expense(&submitted_expense(unsuggested_blob.id)).await.unwrap();
        let already_suggested = ExpenseRecord {
            suggested_amount: Some(42.0),
            ..submitted_expense(suggested_blob.id)
        };
        db.insert_expense(&already_suggested).await.unwrap();

        let (tx, rx) = async_channel::bounded(10);
        let (gtx, _) = async_channel::bounded(10);
        let (rtx, _) = async_channel::bounded(10);
        requeue_stale(&db, &tx, &gtx, &rtx).await.unwrap();

        assert_eq!(rx.len(), 1, "only the unsuggested expense's blob should requeue");
        assert_eq!(
            rx.recv().await.unwrap(),
            PipelineJob::ExpenseSuggestions(unsuggested_blob.id)
        );
    }

    /// Geocoding and routing must not wait out the blob queue. A wedged blob
    /// channel (nothing consuming, capacity 1) blocks that drain indefinitely;
    /// the other two still have to complete. Run sequentially, this hangs —
    /// which is the hours-long startup delay a real backlog produces, since
    /// #406 put retryable failures at the head of that line.
    #[tokio::test]
    async fn test_a_wedged_blob_queue_does_not_hold_geocoding_and_routing() {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        let now = Utc::now();

        for i in 0..4 {
            let mut b = ready_blob(&format!("wedge-{i}"));
            b.status = BlobStatus::Pending;
            b.summary = None;
            db.insert(&b).await.unwrap();
        }
        let facility = crate::models::FacilityRecord {
            id: Uuid::new_v4(), owner_id: 0,
            name: "Shipper".into(), address: "1 Dock St".into(),
            normalized_address: None, lat: None, lng: None,
            geocode_status: crate::models::GeocodeStatus::Pending, geocode_failure_count: 0,
            contacts: vec![], notes: None, tags: vec![],
            blob_ids: vec![], avg_dwell_minutes: None, dwell_sample_count: 0,
            archived: false, embedding: None, created_at: now, updated_at: now,
        };
        db.insert_facility(&facility).await.unwrap();

        // Capacity 1, no consumer: the blob drain wedges on its second send.
        let (tx, _rx_held) = async_channel::bounded(1);
        let (gtx, grx) = async_channel::bounded(10);
        let (rtx, _rrx) = async_channel::bounded(10);

        let wedged = tokio::spawn(async move {
            let dir = dir;
            requeue_stale(&db, &tx, &gtx, &rtx).await.ok();
            drop(dir);
        });

        // The geocode queue must fill while the blob drain is still stuck.
        let got = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if grx.len() == 1 { return; }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await;
        assert!(
            got.is_ok(),
            "facility geocoding must requeue while the blob queue is wedged"
        );
        wedged.abort();
    }

    /// A blob already queued for a full pass must not also get a suggestions
    /// job — `process_blob` stages suggestions itself, so both would run the
    /// extraction model twice on the same receipt.
    #[tokio::test]
    async fn test_requeue_does_not_double_queue_pending_receipt() {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();

        let pending_blob = BlobRecord {
            status: BlobStatus::Pending,
            summary: None,
            ..ready_blob("c-pending")
        };
        db.insert(&pending_blob).await.unwrap();
        db.insert_expense(&submitted_expense(pending_blob.id)).await.unwrap();

        let (tx, rx) = async_channel::bounded(10);
        let (gtx, _) = async_channel::bounded(10);
        let (rtx, _) = async_channel::bounded(10);
        requeue_stale(&db, &tx, &gtx, &rtx).await.unwrap();

        assert_eq!(rx.len(), 1, "expected exactly one job for the pending receipt");
        assert_eq!(rx.recv().await.unwrap(), PipelineJob::Process(pending_blob.id));
    }
}
