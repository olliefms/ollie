// src/pipeline/recovery.rs
use crate::{db::DbClient, error::AppError};
use uuid::Uuid;

pub async fn requeue_stale(
    db: &DbClient,
    tx: &async_channel::Sender<Uuid>,
) -> Result<usize, AppError> {
    let ids = db.list_non_ready_ids().await?;
    let count = ids.len();
    for id in ids {
        tx.send(id).await.map_err(|e| AppError::Internal(e.to_string()))?;
    }
    tracing::info!("requeued {count} stale blobs on startup");
    Ok(count)
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
        };
        let ready = BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum: "c2".into(),
            name: "g.txt".into(), mime_type: "text/plain".into(), size: 1,
            status: BlobStatus::Ready, error: None, summary: None,
            tags: vec![], embedding: None, created_at: now, updated_at: now,
        };
        db.insert(&pending).await.unwrap();
        db.insert(&ready).await.unwrap();

        let (tx, rx) = async_channel::bounded(10);
        let count = requeue_stale(&db, &tx).await.unwrap();
        assert_eq!(count, 1);
        let received = rx.recv().await.unwrap();
        assert_eq!(received, pending.id);
    }
}
