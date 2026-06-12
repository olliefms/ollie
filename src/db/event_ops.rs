// src/db/event_ops.rs
use crate::{
    ai::{embed::embed_text, OllamaClient},
    db::{event_schema, DbClient},
    error::AppError,
    models::EventRecord,
};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator,
    RecordBatchReader, StringArray,
};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;
use uuid::Uuid;

/// Upper bound on rows scanned for a *filtered* event query (entity/type/time
/// scoped). Such result sets are small (one entity's history), so we fetch a
/// capped superset and order it in memory. If a filtered query ever matches more
/// than this many rows, only the first `EVENT_SCAN_CAP` in scan (insertion) order
/// are considered — a per-entity history that large is not expected in practice.
const EVENT_SCAN_CAP: usize = 5_000;

impl DbClient {
    #[allow(clippy::too_many_arguments)]
    pub async fn append_event(
        &self,
        entity_type: &str,
        entity_id: Uuid,
        event_type: &str,
        payload: Option<serde_json::Value>,
        actor: Option<&str>,
        occurred_at: &str,
        ai: Option<&OllamaClient>,
    ) -> Result<Uuid, AppError> {
        chrono::DateTime::parse_from_rfc3339(occurred_at)
            .map_err(|_| AppError::BadRequest("occurred_at must be RFC3339 UTC+Z".into()))?;
        if !occurred_at.ends_with('Z') {
            return Err(AppError::BadRequest("occurred_at must be RFC3339 UTC+Z".into()));
        }

        let id = Uuid::new_v4();
        let payload_str = payload.as_ref().map(|v| v.to_string());

        let embedding = if let Some(client) = ai {
            let payload_snippet = payload_str.as_deref().unwrap_or("");
            let embed_src = format!(
                "{entity_type} {event_type} {}",
                &payload_snippet[..payload_snippet.len().min(500)],
            );
            embed_text(client, &embed_src).await.ok()
        } else {
            None
        };

        let record = EventRecord {
            id,
            entity_type: entity_type.to_string(),
            entity_id,
            event_type: event_type.to_string(),
            payload: payload_str,
            actor: actor.map(|s| s.to_string()),
            occurred_at: occurred_at.to_string(),
            embedding,
        };

        let batch = event_to_batch(&record, self.embed_dim)?;
        let schema = event_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.event_table.add(reader).execute().await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))?;

        Ok(id)
    }

    pub async fn get_event(&self, id: Uuid) -> Result<EventRecord, AppError> {
        let stream = self.event_table.query()
            .only_if(format!("id = '{id}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        batches_to_events(collect_stream(stream).await?)?
            .into_iter().next()
            .ok_or(AppError::NotFound)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn query_events(
        &self,
        entity_id: Option<Uuid>,
        entity_type: Option<&str>,
        event_type: Option<&str>,
        from: Option<&str>,
        to: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(usize, Vec<EventRecord>), AppError> {
        if offset > 10_000 {
            return Err(AppError::BadRequest("offset must not exceed 10000".into()));
        }
        let filter = build_event_filter(entity_id, entity_type, event_type, from, to)?;
        let total = self.event_table.count_rows(filter.clone()).await
            .map_err(|e| AppError::Internal(e.to_string()))?;

        // LanceDB has no ORDER BY for scalar scans. The previous implementation
        // applied LIMIT before sorting, so it returned the OLDEST `limit` rows
        // and sorted only those among themselves — hiding all recent activity.
        //
        // The events table is append-only, so scan order tracks insertion order
        // (ascending occurred_at). For the unfiltered feed we therefore fetch the
        // TAIL of the scan — the newest `limit + offset` rows — which is O(page)
        // regardless of table size. Filtered queries (entity/type/time) match a
        // small set, so we scan a capped superset. Either window is then sorted
        // descending and paginated in memory.
        let (scan_offset, scan_limit) = if filter.is_some() {
            (0, EVENT_SCAN_CAP)
        } else {
            let window = limit.saturating_add(offset);
            (total.saturating_sub(window), window)
        };
        let mut q = self.event_table.query().offset(scan_offset).limit(scan_limit);
        if let Some(f) = filter { q = q.only_if(f); }
        let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let mut all = batches_to_events(collect_stream(stream).await?)?;
        all.sort_by(|a, b| b.occurred_at.cmp(&a.occurred_at));
        let items = all.into_iter().skip(offset).take(limit).collect();
        Ok((total, items))
    }

    pub async fn create_event_vector_index(&self) -> Result<(), AppError> {
        self.create_ivfpq_index(&self.event_table, "embedding", "events").await
    }

    pub async fn create_event_scalar_indices(&self) -> Result<(), AppError> {
        for col in ["entity_id", "occurred_at", "event_type"] {
            self.event_table
                .create_index(&[col], lancedb::index::Index::BTree(Default::default()))
                .execute().await
                .map_err(|e| AppError::Internal(e.to_string()))?;
        }
        Ok(())
    }
}

// --- Helpers ---

fn event_to_batch(record: &EventRecord, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let schema = event_schema(embed_dim);

    let id_str = record.id.to_string();
    let entity_id_str = record.entity_id.to_string();

    let embedding_col: Arc<dyn arrow_array::Array> = match &record.embedding {
        Some(v) => {
            let floats: Vec<Option<f32>> = v.iter().map(|&f| Some(f)).collect();
            Arc::new(FixedSizeListArray::from_iter_primitive::<
                arrow_array::types::Float32Type, _, _
            >(vec![Some(floats)], embed_dim as i32))
        }
        None => Arc::new(FixedSizeListArray::from_iter_primitive::<
            arrow_array::types::Float32Type, _, _
        >(vec![None::<Vec<Option<f32>>>], embed_dim as i32)),
    };

    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(vec![id_str.as_str()])),
        Arc::new(StringArray::from(vec![record.entity_type.as_str()])),
        Arc::new(StringArray::from(vec![entity_id_str.as_str()])),
        Arc::new(StringArray::from(vec![record.event_type.as_str()])),
        Arc::new(StringArray::from(vec![record.payload.as_deref()])),
        Arc::new(StringArray::from(vec![record.actor.as_deref()])),
        Arc::new(StringArray::from(vec![record.occurred_at.as_str()])),
        embedding_col,
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_events(batches: Vec<RecordBatch>) -> Result<Vec<EventRecord>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() { out.push(row_to_event(batch, i)?); }
    }
    Ok(out)
}

fn row_to_event(batch: &RecordBatch, i: usize) -> Result<EventRecord, AppError> {
    let str_col = |name: &str| -> String {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .map(|a| a.value(i).to_string()).unwrap_or_default()
    };
    let opt_str = |name: &str| -> Option<String> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i).to_string()) })
    };

    let embedding = batch.column_by_name("embedding")
        .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
        .and_then(|fsl| {
            if fsl.is_null(i) { return None; }
            let values = fsl.value(i);
            values.as_any().downcast_ref::<Float32Array>()
                .map(|fa| (0..fa.len()).map(|j| fa.value(j)).collect::<Vec<f32>>())
        });

    Ok(EventRecord {
        id: str_col("id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        entity_type: str_col("entity_type"),
        entity_id: str_col("entity_id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        event_type: str_col("event_type"),
        payload: opt_str("payload"),
        actor: opt_str("actor"),
        occurred_at: str_col("occurred_at"),
        embedding,
    })
}

fn build_event_filter(
    entity_id: Option<Uuid>,
    entity_type: Option<&str>,
    event_type: Option<&str>,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<Option<String>, AppError> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(id) = entity_id {
        parts.push(format!("entity_id = '{id}'"));
    }
    if let Some(et) = entity_type {
        parts.push(format!("entity_type = '{}'", et.replace('\'', "''")));
    }
    if let Some(evt) = event_type {
        parts.push(format!("event_type = '{}'", evt.replace('\'', "''")));
    }
    if let Some(f) = from {
        chrono::DateTime::parse_from_rfc3339(f)
            .map_err(|_| AppError::BadRequest("invalid 'from' datetime".into()))?;
        parts.push(format!("occurred_at >= '{f}'"));
    }
    if let Some(t) = to {
        chrono::DateTime::parse_from_rfc3339(t)
            .map_err(|_| AppError::BadRequest("invalid 'to' datetime".into()))?;
        parts.push(format!("occurred_at <= '{t}'"));
    }
    Ok(if parts.is_empty() { None } else { Some(parts.join(" AND ")) })
}

async fn collect_stream(
    stream: impl futures::TryStream<Ok = RecordBatch, Error = impl std::error::Error + Send + Sync + 'static> + Send,
) -> Result<Vec<RecordBatch>, AppError> {
    stream.try_collect::<Vec<_>>().await.map_err(|e| AppError::Internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn test_db() -> (DbClient, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        (db, dir)
    }

    fn now_rfc3339() -> String {
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    #[tokio::test]
    async fn test_append_and_get_event() {
        let (db, _dir) = test_db().await;
        let entity_id = Uuid::new_v4();
        let id = db.append_event(
            "blob", entity_id, "processing_started",
            None, Some("pipeline"), &now_rfc3339(), None,
        ).await.unwrap();
        let record = db.get_event(id).await.unwrap();
        assert_eq!(record.id, id);
        assert_eq!(record.entity_type, "blob");
        assert_eq!(record.entity_id, entity_id);
        assert_eq!(record.event_type, "processing_started");
        assert_eq!(record.actor.as_deref(), Some("pipeline"));
    }

    #[tokio::test]
    async fn test_get_event_not_found() {
        let (db, _dir) = test_db().await;
        assert!(matches!(db.get_event(Uuid::new_v4()).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_append_event_invalid_occurred_at() {
        let (db, _dir) = test_db().await;
        let result = db.append_event(
            "blob", Uuid::new_v4(), "test",
            None, None, "not-a-date", None,
        ).await;
        assert!(matches!(result, Err(AppError::BadRequest(_))));
    }

    #[tokio::test]
    async fn test_query_events_by_entity_id() {
        let (db, _dir) = test_db().await;
        let entity_id = Uuid::new_v4();
        let other_id = Uuid::new_v4();
        db.append_event("blob", entity_id, "processing_started", None, None, &now_rfc3339(), None).await.unwrap();
        db.append_event("blob", entity_id, "processing_completed", None, None, &now_rfc3339(), None).await.unwrap();
        db.append_event("blob", other_id, "processing_started", None, None, &now_rfc3339(), None).await.unwrap();
        let (total, items) = db.query_events(Some(entity_id), None, None, None, None, 100, 0).await.unwrap();
        assert_eq!(total, 2);
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|r| r.entity_id == entity_id));
    }

    #[tokio::test]
    async fn test_query_events_by_event_type() {
        let (db, _dir) = test_db().await;
        let entity_id = Uuid::new_v4();
        db.append_event("blob", entity_id, "processing_started", None, None, &now_rfc3339(), None).await.unwrap();
        db.append_event("blob", entity_id, "processing_failed", None, None, &now_rfc3339(), None).await.unwrap();
        let (total, items) = db.query_events(None, None, Some("processing_started"), None, None, 100, 0).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].event_type, "processing_started");
    }

    // The feed must return the NEWEST events, not the oldest: insert more events
    // than the page size and assert we get the most-recent ones, newest-first.
    #[tokio::test]
    async fn test_query_events_unfiltered_returns_newest_first() {
        let (db, _dir) = test_db().await;
        for i in 0..25u32 {
            let ts = format!("2026-01-01T00:{i:02}:00.000Z");
            db.append_event("trip", Uuid::new_v4(), "stop.arrived", None, None, &ts, None).await.unwrap();
        }
        let (total, items) = db.query_events(None, None, None, None, None, 10, 0).await.unwrap();
        assert_eq!(total, 25);
        assert_eq!(items.len(), 10);
        // Newest first: minute 24 down to minute 15.
        assert_eq!(items[0].occurred_at, "2026-01-01T00:24:00.000Z");
        assert_eq!(items[9].occurred_at, "2026-01-01T00:15:00.000Z");
        for w in items.windows(2) {
            assert!(w[0].occurred_at >= w[1].occurred_at, "must be descending");
        }
    }

    #[tokio::test]
    async fn test_query_events_unfiltered_pagination() {
        let (db, _dir) = test_db().await;
        for i in 0..25u32 {
            let ts = format!("2026-01-01T00:{i:02}:00.000Z");
            db.append_event("trip", Uuid::new_v4(), "stop.arrived", None, None, &ts, None).await.unwrap();
        }
        // Second page of 5 (skip the 5 newest) -> minutes 19..15.
        let (total, items) = db.query_events(None, None, None, None, None, 5, 5).await.unwrap();
        assert_eq!(total, 25);
        assert_eq!(items.len(), 5);
        assert_eq!(items[0].occurred_at, "2026-01-01T00:19:00.000Z");
        assert_eq!(items[4].occurred_at, "2026-01-01T00:15:00.000Z");
    }

    #[tokio::test]
    async fn test_query_events_entity_filter_newest_first() {
        let (db, _dir) = test_db().await;
        let trip = Uuid::new_v4();
        // Noise from another entity, plus 12 events for `trip`, all ascending.
        for i in 0..8u32 {
            let ts = format!("2026-01-01T00:{i:02}:00.000Z");
            db.append_event("trip", Uuid::new_v4(), "stop.arrived", None, None, &ts, None).await.unwrap();
        }
        for i in 30..42u32 {
            let ts = format!("2026-01-01T00:{i:02}:00.000Z");
            db.append_event("trip", trip, "stop.arrived", None, None, &ts, None).await.unwrap();
        }
        let (total, items) = db.query_events(Some(trip), None, None, None, None, 5, 0).await.unwrap();
        assert_eq!(total, 12);
        assert_eq!(items.len(), 5);
        assert!(items.iter().all(|r| r.entity_id == trip));
        // Newest first: minute 41 down to 37.
        assert_eq!(items[0].occurred_at, "2026-01-01T00:41:00.000Z");
        assert_eq!(items[4].occurred_at, "2026-01-01T00:37:00.000Z");
    }

    #[test]
    fn test_build_event_filter_invalid_from() {
        let result = build_event_filter(None, None, None, Some("not-a-date"), None);
        assert!(matches!(result, Err(AppError::BadRequest(_))));
    }

    #[test]
    fn test_build_event_filter_invalid_to() {
        let result = build_event_filter(None, None, None, None, Some("' OR 1=1--"));
        assert!(matches!(result, Err(AppError::BadRequest(_))));
    }

    #[test]
    fn test_build_event_filter_injection_in_entity_type() {
        let filter = build_event_filter(None, Some("blob' OR '1'='1"), None, None, None).unwrap();
        let f = filter.unwrap();
        assert!(f.contains("blob'' OR ''1''=''1"));
    }
}
