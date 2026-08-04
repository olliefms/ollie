// src/pipeline/mod.rs
pub mod embedding_backfill;
pub mod geocoding;
pub mod recovery;
pub mod routing;
pub mod worker;

use crate::{ai::OllamaClient, db::DbClient, error::AppError, storage::BlobStore};
use futures::FutureExt;
use std::sync::Arc;
use uuid::Uuid;

/// A unit of work for the document AI pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipelineJob {
    /// Full pass: extract, summarize, embed, mark ready, then stage expense
    /// suggestions for any submitted expense referencing the blob.
    Process(Uuid),
    /// Suggestions-only pass for a blob that is already Ready (dedup re-upload
    /// of an expense receipt): the summary/embedding were copied from the
    /// existing record at upload time, so only receipt-field extraction runs.
    /// Blob status and events are untouched.
    ExpenseSuggestions(Uuid),
}

/// Hand a job to the pipeline without making a request handler wait for a free
/// slot.
///
/// The channel is bounded, so a saturated pipeline — a bulk import, or the
/// startup backlog drain, which the API now serves through (#404) — would
/// otherwise block the upload for as long as the drain takes, and a client
/// timeout would cancel the send outright, leaving a `Pending` blob with no
/// queued job until the next restart. The record is always persisted before the
/// enqueue, so detaching costs at worst a late summary.
///
/// The `try_send` fast path keeps the common case synchronous. Only a *full*
/// channel detaches; a *closed* one is returned as an error, because it means
/// every pipeline worker is gone and no amount of waiting will queue anything —
/// reporting success there would make the 202 (or `queued: true`) a false claim.
/// Startup recovery deliberately does NOT use this — it wants the backpressure,
/// not 768 parked tasks.
pub fn enqueue(tx: &async_channel::Sender<PipelineJob>, job: PipelineJob) -> Result<(), AppError> {
    match tx.try_send(job) {
        Ok(()) => Ok(()),
        Err(async_channel::TrySendError::Closed(_)) => Err(AppError::Internal(
            "pipeline channel is closed — no workers are running".into(),
        )),
        Err(async_channel::TrySendError::Full(job)) => {
            let tx = tx.clone();
            tokio::spawn(async move {
                if let Err(e) = tx.send(job).await {
                    tracing::error!("pipeline enqueue failed for {job:?}: {e}");
                }
            });
            Ok(())
        }
    }
}

/// Run one job so a panic in it cannot escape into the worker loop.
///
/// The blob pipeline runs third-party parsers over arbitrary uploaded bytes,
/// and those panic rather than error on malformed input (`pdf-extract` and the
/// image decoders both do). A panic unwinds the whole worker *task*, dropping
/// its receiver — and with `PIPELINE_WORKERS` defaulting to 1 that is the last
/// receiver, so the channel closes and every later `enqueue` fails with
/// "sending into a closed channel" for the lifetime of the process while the
/// HTTP server carries on serving. One poisoned upload must cost one blob, not
/// the pipeline.
async fn run_job<F>(fut: F) -> Result<Result<(), AppError>, ()>
where
    F: std::future::Future<Output = Result<(), AppError>>,
{
    std::panic::AssertUnwindSafe(fut).catch_unwind().await.map_err(|_| ())
}

pub fn spawn_pipeline(
    workers: usize,
    db: Arc<DbClient>,
    store: Arc<BlobStore>,
    ai: Arc<OllamaClient>,
    extract_base: String,
) -> async_channel::Sender<PipelineJob> {
    let workers = workers.max(1);
    let (tx, rx) = async_channel::bounded::<PipelineJob>(256);

    for i in 0..workers {
        let rx = rx.clone();
        let db = db.clone();
        let store = store.clone();
        let ai = ai.clone();
        let extract_base = extract_base.clone();
        tokio::spawn(async move {
            tracing::info!("pipeline worker {i} started");
            while let Ok(job) = rx.recv().await {
                let (PipelineJob::Process(id) | PipelineJob::ExpenseSuggestions(id)) = job;
                let result = run_job(async {
                    match job {
                        PipelineJob::Process(id) => {
                            worker::process_blob(id, &db, &store, &ai, &extract_base).await
                        }
                        PipelineJob::ExpenseSuggestions(id) => {
                            worker::stage_expense_suggestions(id, &db, &store, &ai).await
                        }
                    }
                })
                .await;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => tracing::error!("worker {i} error for {id}: {e}"),
                    Err(()) => {
                        // The panic itself is already on stderr via the default
                        // hook; what matters here is that the worker lives and
                        // the blob does not sit in Processing for ever.
                        tracing::error!("worker {i} panicked on {id}; worker survives, blob marked failed");
                        if let Err(e) = db.mark_failed(id, "pipeline worker panicked".into()).await {
                            tracing::error!("could not mark {id} failed after panic: {e}");
                        }
                    }
                }
            }
            tracing::info!("pipeline worker {i} stopped");
        });
    }
    tx
}

pub fn spawn_geocoding_pipeline(
    workers: usize,
    db: Arc<DbClient>,
    geocoding: Arc<crate::geocoding::GeocodingClient>,
    ai: Arc<crate::ai::OllamaClient>,
    routing_tx: async_channel::Sender<uuid::Uuid>,
) -> async_channel::Sender<uuid::Uuid> {
    let workers = workers.max(1);
    let (tx, rx) = async_channel::bounded::<uuid::Uuid>(256);
    for i in 0..workers {
        let rx = rx.clone();
        let db = db.clone();
        let geocoding = geocoding.clone();
        let ai = ai.clone();
        let routing_tx = routing_tx.clone();
        tokio::spawn(async move {
            tracing::info!("geocoding worker {i} started");
            while let Ok(id) = rx.recv().await {
                match run_job(geocoding::process_facility_geocoding(id, &db, &geocoding, &ai, &routing_tx)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => tracing::error!("geocoding worker {i} error for {id}: {e}"),
                    Err(()) => tracing::error!("geocoding worker {i} panicked on {id}; worker survives"),
                }
            }
        });
    }
    tx
}

pub fn spawn_routing_pipeline(
    workers: usize,
    db: Arc<DbClient>,
    ors: Arc<crate::routing::RoutingClient>,
) -> async_channel::Sender<uuid::Uuid> {
    let workers = workers.max(1);
    let (tx, rx) = async_channel::bounded::<uuid::Uuid>(256);
    for i in 0..workers {
        let rx = rx.clone();
        let db = db.clone();
        let ors = ors.clone();
        tokio::spawn(async move {
            tracing::info!("routing worker {i} started");
            while let Ok(id) = rx.recv().await {
                match run_job(routing::process_load_routing(id, &db, &ors)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => tracing::error!("routing worker {i} error for {id}: {e}"),
                    Err(()) => tracing::error!("routing worker {i} panicked on {id}; worker survives"),
                }
            }
        });
    }
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full channel must not block the caller, and must not lose the job.
    #[tokio::test]
    async fn test_enqueue_detaches_when_full() {
        let (tx, rx) = async_channel::bounded::<PipelineJob>(1);
        let first = PipelineJob::Process(Uuid::new_v4());
        let second = PipelineJob::Process(Uuid::new_v4());
        enqueue(&tx, first).unwrap();

        // Channel is now full; this one has to detach rather than block.
        enqueue(&tx, second).unwrap();
        assert_eq!(rx.recv().await.unwrap(), first);
        assert_eq!(rx.recv().await.unwrap(), second, "the detached job was lost");
    }

    /// A closed channel means every worker is gone — no amount of waiting will
    /// queue anything, so the caller must hear about it rather than get a 202
    /// that claims work was scheduled.
    /// A panicking job must not unwind the worker loop — that is what closes the
    /// channel and takes the whole pipeline down until the next restart.
    #[tokio::test]
    async fn test_run_job_contains_a_panic() {
        assert!(run_job(async { panic!("pdf-extract style panic") }).await.is_err());
        assert!(run_job(async { Ok(()) }).await.unwrap().is_ok());
        assert!(run_job(async { Err(AppError::Internal("boom".into())) }).await.unwrap().is_err());
    }

    #[tokio::test]
    async fn test_enqueue_errors_when_closed() {
        let (tx, rx) = async_channel::bounded::<PipelineJob>(1);
        drop(rx);
        assert!(enqueue(&tx, PipelineJob::Process(Uuid::new_v4())).is_err());
    }
}
