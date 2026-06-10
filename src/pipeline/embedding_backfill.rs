// src/pipeline/embedding_backfill.rs
//
// Facilities are embedded best-effort at create time. If the embed model is
// unavailable then (e.g. Ollama up but `nomic-embed-text` not pulled), the
// facility is persisted with `embedding: None` and becomes invisible to
// semantic dedup. The geocoding pipeline only re-embeds after a *successful*
// geocode, so facilities that fail to geocode — or that were created with
// explicit coords and never queued for geocoding — never recover.
//
// This module sweeps `embedding IS NULL` facilities once at startup and embeds
// them, mirroring the startup recovery cadence in `pipeline::recovery`.

use crate::{ai::embed::embed_text, ai::OllamaClient, db::DbClient};
use std::sync::Arc;

/// Embed every facility currently missing an embedding. Best-effort: an embed
/// failure for one facility is logged and skipped (it will be retried on the
/// next startup). Returns the number of facilities successfully embedded.
pub async fn backfill_facility_embeddings(db: &DbClient, ai: &OllamaClient) -> usize {
    let ids = match db.list_facilities_missing_embedding().await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!("facility embedding backfill: query failed: {e}");
            return 0;
        }
    };
    if ids.is_empty() {
        return 0;
    }
    tracing::info!("backfilling embeddings for {} facilities", ids.len());

    let mut embedded = 0;
    for id in ids {
        let facility = match db.get_facility_by_id(id).await {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("backfill: failed to load facility {id}: {e}");
                continue;
            }
        };
        match embed_text(ai, &facility.embedding_text()).await {
            Ok(embedding) => match db.update_facility_embedding(id, embedding).await {
                Ok(()) => embedded += 1,
                Err(e) => tracing::warn!("backfill: failed to persist embedding for {id}: {e}"),
            },
            Err(e) => tracing::warn!("backfill: embedding failed for facility {id}: {e}"),
        }
    }
    if embedded > 0 {
        tracing::info!("facility embedding backfill embedded {embedded} facilities");
    }
    embedded
}

/// Spawn [`backfill_facility_embeddings`] as a background task so it does not
/// block startup on Ollama round-trips.
pub fn spawn_facility_embedding_backfill(db: Arc<DbClient>, ai: Arc<OllamaClient>) {
    tokio::spawn(async move {
        backfill_facility_embeddings(&db, &ai).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{FacilityRecord, GeocodeStatus};
    use tempfile::TempDir;

    async fn test_db() -> (DbClient, TempDir) {
        let dir = TempDir::new().unwrap();
        // embed_dim 768 matches nomic-embed-text, so a real embedding (if a
        // local Ollama answers) round-trips instead of panicking on a dim
        // mismatch. With no Ollama reachable, embeds fail and rows stay None.
        let db = DbClient::new(dir.path().to_str().unwrap(), 768).await.unwrap();
        (db, dir)
    }

    fn unreachable_ai() -> OllamaClient {
        // Port 1 refuses immediately, so embed_text fails fast and the backfill
        // is deterministic regardless of whether a local Ollama is running.
        OllamaClient::new("http://127.0.0.1:1", "nomic-embed-text", "llama3.2", "llava")
    }

    fn facility_missing_embedding() -> FacilityRecord {
        let now = chrono::Utc::now();
        FacilityRecord {
            id: uuid::Uuid::new_v4(), owner_id: 0,
            name: "Dock".into(), address: "Memphis, TN".into(),
            normalized_address: None, lat: None, lng: None,
            geocode_status: GeocodeStatus::Pending, geocode_failure_count: 0,
            contacts: vec![], notes: None, tags: vec![], blob_ids: vec![],
            avg_dwell_minutes: None, dwell_sample_count: 0, archived: false,
            embedding: None, created_at: now, updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_backfill_no_missing_returns_zero() {
        let (db, _dir) = test_db().await;
        assert_eq!(backfill_facility_embeddings(&db, &unreachable_ai()).await, 0);
    }

    #[tokio::test]
    async fn test_backfill_leaves_row_intact_when_embed_unavailable() {
        let (db, _dir) = test_db().await;
        let f = facility_missing_embedding();
        db.insert_facility(&f).await.unwrap();

        // Embed model unreachable -> nothing embedded, row stays None (never a
        // partial/corrupt embedding), and it remains queued for the next sweep.
        let embedded = backfill_facility_embeddings(&db, &unreachable_ai()).await;
        assert_eq!(embedded, 0);
        assert!(db.get_facility_by_id(f.id).await.unwrap().embedding.is_none());
        assert_eq!(db.list_facilities_missing_embedding().await.unwrap(), vec![f.id]);
    }
}
