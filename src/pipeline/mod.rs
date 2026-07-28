// src/pipeline/mod.rs
pub mod embedding_backfill;
pub mod geocoding;
pub mod recovery;
pub mod routing;
pub mod worker;

use crate::{ai::OllamaClient, db::DbClient, storage::BlobStore};
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
                let (id, result) = match job {
                    PipelineJob::Process(id) => {
                        (id, worker::process_blob(id, &db, &store, &ai, &extract_base).await)
                    }
                    PipelineJob::ExpenseSuggestions(id) => {
                        (id, worker::stage_expense_suggestions(id, &db, &store, &ai).await)
                    }
                };
                if let Err(e) = result {
                    tracing::error!("worker {i} error for {id}: {e}");
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
                if let Err(e) = geocoding::process_facility_geocoding(id, &db, &geocoding, &ai, &routing_tx).await {
                    tracing::error!("geocoding worker {i} error for {id}: {e}");
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
                if let Err(e) = routing::process_load_routing(id, &db, &ors).await {
                    tracing::error!("routing worker {i} error for {id}: {e}");
                }
            }
        });
    }
    tx
}
