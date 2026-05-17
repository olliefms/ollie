// src/pipeline/recovery.rs
use crate::{db::DbClient, error::AppError};
use uuid::Uuid;

pub async fn requeue_stale(
    db: &DbClient,
    pipeline_tx: &async_channel::Sender<Uuid>,
    geocoding_tx: &async_channel::Sender<Uuid>,
    routing_tx: &async_channel::Sender<Uuid>,
) -> Result<(), AppError> {
    let ids = db.list_non_ready_ids().await?;
    tracing::info!("requeueing {} stale blobs on startup", ids.len());
    for id in ids {
        pipeline_tx.send(id).await.map_err(|e| AppError::Internal(e.to_string()))?;
    }

    let pending_geocode = db.list_pending_geocode_facility_ids().await?;
    tracing::info!("requeueing {} facilities for geocoding", pending_geocode.len());
    for id in pending_geocode {
        geocoding_tx.send(id).await.map_err(|e| AppError::Internal(e.to_string()))?;
    }

    let pending_routing = db.list_loads_needing_routing().await?;
    tracing::info!("requeueing {} loads for routing", pending_routing.len());
    for id in pending_routing {
        routing_tx.send(id).await.map_err(|e| AppError::Internal(e.to_string()))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::DbClient,
        models::{BlobRecord, BlobStatus},
    };
    use chrono::Utc;
    use tempfile::TempDir;
    use uuid::Uuid;

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
        };
        let ready = BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum: "c2".into(),
            name: "g.txt".into(), mime_type: "text/plain".into(), size: 1,
            status: BlobStatus::Ready, error: None, summary: None,
            tags: vec![], embedding: None, created_at: now, updated_at: now,
            visibility: Default::default(), uploaded_by: None,
        };
        db.insert(&pending).await.unwrap();
        db.insert(&ready).await.unwrap();

        let (tx, rx) = async_channel::bounded(10);
        let (gtx, _) = async_channel::bounded(10);
        let (rtx, _) = async_channel::bounded(10);
        requeue_stale(&db, &tx, &gtx, &rtx).await.unwrap();
        assert_eq!(rx.len(), 1);
        let received = rx.recv().await.unwrap();
        assert_eq!(received, pending.id);
    }
}
