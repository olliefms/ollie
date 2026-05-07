use crate::{
    db::{blob_schema, DbClient},
    error::AppError,
    models::{BlobListItem, BlobRecord, BlobStatus},
};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, Int64Array, RecordBatch,
    RecordBatchIterator, StringArray,
};
use arrow_array::RecordBatchReader;
use chrono::Utc;
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;
use uuid::Uuid;

impl DbClient {
    pub async fn insert(&self, record: &BlobRecord) -> Result<(), AppError> {
        let batch = record_to_batch(record, self.embed_dim)?;
        let schema = blob_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.blob_table.add(reader).execute().await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_by_id(&self, id: Uuid) -> Result<BlobRecord, AppError> {
        let id_str = id.to_string();
        let stream = self.blob_table.query()
            .only_if(format!("id = '{id_str}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let batches = collect_stream(stream).await?;
        batches_to_records(batches)?
            .into_iter().next()
            .ok_or(AppError::NotFound)
    }

    pub async fn count_by_checksum(&self, checksum: &str) -> Result<usize, AppError> {
        self.blob_table.count_rows(Some(format!("checksum = '{checksum}'")))
            .await.map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_one_by_checksum(&self, checksum: &str) -> Result<Option<BlobRecord>, AppError> {
        let stream = self.blob_table.query()
            .only_if(format!("checksum = '{checksum}'"))
            .limit(1)
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(batches_to_records(collect_stream(stream).await?)?.into_iter().next())
    }

    pub async fn mark_processing(&self, id: Uuid) -> Result<(), AppError> {
        let mut record = self.get_by_id(id).await?;
        record.status = BlobStatus::Processing;
        record.updated_at = Utc::now();
        self.upsert_blob(&record).await
    }

    pub async fn mark_ready(&self, id: Uuid, summary: Option<String>, embedding: Option<Vec<f32>>) -> Result<(), AppError> {
        let mut record = self.get_by_id(id).await?;
        record.status = BlobStatus::Ready;
        record.summary = summary;
        record.embedding = embedding;
        record.error = None;
        record.updated_at = Utc::now();
        self.upsert_blob(&record).await
    }

    pub async fn mark_failed(&self, id: Uuid, error: String) -> Result<(), AppError> {
        let mut record = self.get_by_id(id).await?;
        record.status = BlobStatus::Failed;
        record.error = Some(error);
        record.updated_at = Utc::now();
        self.upsert_blob(&record).await
    }

    pub async fn update_metadata(&self, id: Uuid, name: Option<String>, tags: Option<Vec<String>>) -> Result<BlobRecord, AppError> {
        let mut record = self.get_by_id(id).await?;
        if let Some(n) = name { record.name = n; }
        if let Some(t) = tags { record.tags = t; }
        record.updated_at = Utc::now();
        self.upsert_blob(&record).await?;
        Ok(record)
    }

    async fn upsert_blob(&self, record: &BlobRecord) -> Result<(), AppError> {
        let batch = record_to_batch(record, self.embed_dim)?;
        let schema = blob_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.blob_table.merge_insert(&["id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn delete_by_id(&self, id: Uuid) -> Result<(), AppError> {
        let id_str = id.to_string();
        self.blob_table.delete(&format!("id = '{id_str}'")).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn list(&self, name_filter: Option<&str>, tag_filter: &[String], limit: usize, offset: usize) -> Result<(usize, Vec<BlobListItem>), AppError> {
        let filter = build_filter(name_filter, tag_filter, None);
        let total = self.blob_table.count_rows(filter.clone()).await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let mut q = self.blob_table.query().limit(limit + offset);
        if let Some(f) = filter { q = q.only_if(f); }
        let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let items: Vec<BlobListItem> = batches_to_records(collect_stream(stream).await?)?
            .into_iter()
            .skip(offset)
            .map(BlobListItem::from)
            .collect();
        Ok((total, items))
    }

    pub async fn search(&self, embedding: Vec<f32>, name_filter: Option<&str>, tag_filter: &[String], limit: usize) -> Result<Vec<BlobListItem>, AppError> {
        let filter = build_filter(name_filter, tag_filter, Some("status = 'ready'"));
        let mut q = self.blob_table.query()
            .nearest_to(embedding)
            .map_err(|e| AppError::Internal(e.to_string()))?
            .limit(limit);
        if let Some(f) = filter { q = q.only_if(f); }
        let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let batches = collect_stream(stream).await?;
        let mut items = Vec::new();
        for batch in &batches {
            let distance_col = batch.column_by_name("_distance")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>())
                .map(|a| (0..a.len()).map(|i| a.value(i)).collect::<Vec<f32>>());
            for (i, record) in batches_to_records(vec![batch.clone()])?.into_iter().enumerate() {
                let mut item = BlobListItem::from(record);
                if let Some(ref d) = distance_col {
                    item.score = Some(1.0 / (1.0 + d[i]));
                }
                items.push(item);
            }
        }
        Ok(items)
    }

    pub async fn list_non_ready_ids(&self) -> Result<Vec<Uuid>, AppError> {
        let stream = self.blob_table.query()
            .only_if("status = 'pending' OR status = 'processing'")
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(batches_to_records(collect_stream(stream).await?)?
            .into_iter().map(|r| r.id).collect())
    }

    pub async fn create_vector_index(&self) -> Result<(), AppError> {
        self.blob_table
            .create_index(&["embedding"], lancedb::index::Index::IvfPq(Default::default()))
            .execute()
            .await
            .map_err(|e| AppError::Internal(e.to_string()))
    }
}

// --- Helpers ---

fn record_to_batch(record: &BlobRecord, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let schema = blob_schema(embed_dim);

    let id_str = record.id.to_string();
    let tags_json = serde_json::to_string(&record.tags)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let created = record.created_at.to_rfc3339();
    let updated = record.updated_at.to_rfc3339();
    let status_str = record.status.as_str().to_string();

    let embedding_col: Arc<dyn arrow_array::Array> = match &record.embedding {
        Some(v) => {
            let floats: Vec<Option<f32>> = v.iter().map(|&f| Some(f)).collect();
            Arc::new(FixedSizeListArray::from_iter_primitive::<
                arrow_array::types::Float32Type,
                _,
                _,
            >(vec![Some(floats)], embed_dim as i32))
        }
        None => Arc::new(FixedSizeListArray::from_iter_primitive::<
            arrow_array::types::Float32Type,
            _,
            _,
        >(vec![None::<Vec<Option<f32>>>], embed_dim as i32)),
    };

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(vec![id_str.as_str()])),
            Arc::new(Int64Array::from(vec![record.owner_id])),
            Arc::new(StringArray::from(vec![record.checksum.as_str()])),
            Arc::new(StringArray::from(vec![record.name.as_str()])),
            Arc::new(StringArray::from(vec![record.mime_type.as_str()])),
            Arc::new(Int64Array::from(vec![record.size])),
            Arc::new(StringArray::from(vec![status_str.as_str()])),
            Arc::new(StringArray::from(vec![record.error.as_deref()])),
            Arc::new(StringArray::from(vec![record.summary.as_deref()])),
            Arc::new(StringArray::from(vec![tags_json.as_str()])),
            embedding_col,
            Arc::new(StringArray::from(vec![created.as_str()])),
            Arc::new(StringArray::from(vec![updated.as_str()])),
        ],
    )
    .map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_records(batches: Vec<RecordBatch>) -> Result<Vec<BlobRecord>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() {
            out.push(row_to_record(batch, i)?);
        }
    }
    Ok(out)
}

fn row_to_record(batch: &RecordBatch, i: usize) -> Result<BlobRecord, AppError> {
    let str_col = |name: &str| -> String {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .map(|a| a.value(i).to_string())
            .unwrap_or_default()
    };
    let opt_str_col = |name: &str| -> Option<String> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i).to_string()) })
    };
    let i64_col = |name: &str| -> i64 {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .map(|a| a.value(i))
            .unwrap_or(0)
    };

    let tags: Vec<String> = serde_json::from_str(&str_col("tags")).unwrap_or_default();
    let embedding = batch.column_by_name("embedding")
        .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
        .and_then(|fsl| {
            if fsl.is_null(i) { return None; }
            let values = fsl.value(i);
            values.as_any().downcast_ref::<Float32Array>()
                .map(|fa| (0..fa.len()).map(|j| fa.value(j)).collect::<Vec<f32>>())
        });

    Ok(BlobRecord {
        id: str_col("id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        owner_id: i64_col("owner_id"),
        checksum: str_col("checksum"),
        name: str_col("name"),
        mime_type: str_col("mime_type"),
        size: i64_col("size"),
        status: str_col("status").parse().map_err(|e: String| AppError::Internal(e))?,
        error: opt_str_col("error"),
        summary: opt_str_col("summary"),
        tags,
        embedding,
        created_at: str_col("created_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
        updated_at: str_col("updated_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
    })
}

fn build_filter(name: Option<&str>, tags: &[String], extra: Option<&str>) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(e) = extra { parts.push(e.to_string()); }
    if let Some(n) = name { parts.push(format!("name LIKE '%{n}%'")); }
    for tag in tags { parts.push(format!("tags LIKE '%\"{tag}\"%'")); }
    if parts.is_empty() { None } else { Some(parts.join(" AND ")) }
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

    fn sample_record() -> BlobRecord {
        let now = Utc::now();
        BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum: "abc123".into(),
            name: "test.txt".into(), mime_type: "text/plain".into(), size: 42,
            status: BlobStatus::Pending, error: None, summary: None,
            tags: vec!["tag1".into()], embedding: None,
            created_at: now, updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_insert_and_get_by_id() {
        let (db, _dir) = test_db().await;
        let record = sample_record();
        db.insert(&record).await.unwrap();
        let fetched = db.get_by_id(record.id).await.unwrap();
        assert_eq!(fetched.id, record.id);
        assert_eq!(fetched.name, "test.txt");
        assert_eq!(fetched.tags, vec!["tag1"]);
    }

    #[tokio::test]
    async fn test_get_by_id_not_found() {
        let (db, _dir) = test_db().await;
        assert!(matches!(db.get_by_id(Uuid::new_v4()).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_count_by_checksum_dedup() {
        let (db, _dir) = test_db().await;
        let mut r1 = sample_record();
        let mut r2 = sample_record();
        r2.id = Uuid::new_v4();
        r1.checksum = "shared".into();
        r2.checksum = "shared".into();
        db.insert(&r1).await.unwrap();
        db.insert(&r2).await.unwrap();
        assert_eq!(db.count_by_checksum("shared").await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_mark_ready_sets_embedding_and_summary() {
        let (db, _dir) = test_db().await;
        let record = sample_record();
        db.insert(&record).await.unwrap();
        db.mark_ready(record.id, Some("summary".into()), Some(vec![1.0, 2.0, 3.0, 4.0]))
            .await
            .unwrap();
        let fetched = db.get_by_id(record.id).await.unwrap();
        assert_eq!(fetched.status, BlobStatus::Ready);
        assert_eq!(fetched.summary.as_deref(), Some("summary"));
        assert!(fetched.embedding.is_some());
    }

    #[tokio::test]
    async fn test_mark_failed_stores_error() {
        let (db, _dir) = test_db().await;
        let record = sample_record();
        db.insert(&record).await.unwrap();
        db.mark_failed(record.id, "ollama timeout".into()).await.unwrap();
        let fetched = db.get_by_id(record.id).await.unwrap();
        assert_eq!(fetched.status, BlobStatus::Failed);
        assert_eq!(fetched.error.as_deref(), Some("ollama timeout"));
    }

    #[tokio::test]
    async fn test_mark_processing_preserves_existing_fields() {
        let (db, _dir) = test_db().await;
        let mut record = sample_record();
        record.summary = Some("pre-existing".into());
        record.embedding = Some(vec![0.1, 0.2, 0.3, 0.4]);
        db.insert(&record).await.unwrap();
        db.mark_processing(record.id).await.unwrap();
        let fetched = db.get_by_id(record.id).await.unwrap();
        assert_eq!(fetched.status, BlobStatus::Processing);
        assert_eq!(fetched.summary.as_deref(), Some("pre-existing"),
            "mark_processing must not wipe existing summary");
    }

    #[tokio::test]
    async fn test_delete_by_id() {
        let (db, _dir) = test_db().await;
        let record = sample_record();
        db.insert(&record).await.unwrap();
        db.delete_by_id(record.id).await.unwrap();
        assert!(matches!(db.get_by_id(record.id).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_update_metadata() {
        let (db, _dir) = test_db().await;
        let record = sample_record();
        db.insert(&record).await.unwrap();
        let updated = db.update_metadata(record.id, Some("new.txt".into()), Some(vec!["x".into()]))
            .await.unwrap();
        assert_eq!(updated.name, "new.txt");
        assert_eq!(updated.tags, vec!["x"]);
    }

    #[tokio::test]
    async fn test_list_non_ready_ids() {
        let (db, _dir) = test_db().await;
        let r1 = sample_record();
        let mut r2 = sample_record();
        r2.id = Uuid::new_v4();
        r2.status = BlobStatus::Ready;
        db.insert(&r1).await.unwrap();
        db.insert(&r2).await.unwrap();
        let ids = db.list_non_ready_ids().await.unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], r1.id);
    }
}
