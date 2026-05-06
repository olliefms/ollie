// src/pipeline/mod.rs
pub mod recovery;
pub mod worker;

use crate::{ai::OllamaClient, db::DbClient, storage::BlobStore};
use std::sync::Arc;
use uuid::Uuid;

pub fn spawn_pipeline(
    workers: usize,
    db: Arc<DbClient>,
    store: Arc<BlobStore>,
    ai: Arc<OllamaClient>,
) -> async_channel::Sender<Uuid> {
    let workers = workers.max(1);
    let (tx, rx) = async_channel::bounded::<Uuid>(256);

    for i in 0..workers {
        let rx = rx.clone();
        let db = db.clone();
        let store = store.clone();
        let ai = ai.clone();
        tokio::spawn(async move {
            tracing::info!("pipeline worker {i} started");
            while let Ok(id) = rx.recv().await {
                if let Err(e) = worker::process_blob(id, &db, &store, &ai).await {
                    tracing::error!("worker {i} error for {id}: {e}");
                }
            }
            tracing::info!("pipeline worker {i} stopped");
        });
    }
    tx
}
