// src/db/truck_ops.rs
use crate::{
    db::{truck_schema, DbClient},
    error::AppError,
    models::{TruckListItem, TruckRecord, TruckStatus},
};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, Int64Array,
    RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray,
};
use chrono::Utc;
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;
use uuid::Uuid;

impl DbClient {
    pub async fn insert_truck(&self, record: &TruckRecord) -> Result<(), AppError> {
        let batch = truck_to_batch(record, self.embed_dim)?;
        let schema = truck_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.truck_table.add(reader).execute().await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_truck_by_id(&self, id: Uuid) -> Result<TruckRecord, AppError> {
        let id_str = id.to_string();
        let stream = self.truck_table.query()
            .only_if(format!("id = '{id_str}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        batches_to_trucks(collect_stream(stream).await?)?
            .into_iter().next()
            .ok_or(AppError::NotFound)
    }

    pub async fn batch_get_trucks(
        &self,
        ids: &[uuid::Uuid],
    ) -> Result<std::collections::HashMap<uuid::Uuid, crate::models::TruckRecord>, AppError> {
        if ids.is_empty() { return Ok(std::collections::HashMap::new()); }
        let id_list = ids.iter().map(|id| format!("'{id}'")).collect::<Vec<_>>().join(", ");
        let stream = self.truck_table.query()
            .only_if(format!("id IN ({id_list})"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(batches_to_trucks(collect_stream(stream).await?)?
            .into_iter().map(|r| (r.id, r)).collect())
    }

    async fn upsert_truck(&self, record: &TruckRecord) -> Result<(), AppError> {
        let batch = truck_to_batch(record, self.embed_dim)?;
        let schema = truck_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.truck_table.merge_insert(&["id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_truck_metadata(
        &self, id: Uuid,
        unit_number: Option<String>,
        year: Option<i32>,
        make: Option<String>,
        model: Option<String>,
        vin: Option<String>,
        plate: Option<String>,
        plate_state: Option<String>,
        notes: Option<String>,
        blob_ids: Option<Vec<Uuid>>,
    ) -> Result<TruckRecord, AppError> {
        let mut record = self.get_truck_by_id(id).await?;
        if let Some(v) = unit_number { record.unit_number = v; }
        if let Some(v) = year { record.year = Some(v); }
        if let Some(v) = make { record.make = Some(v); }
        if let Some(v) = model { record.model = Some(v); }
        if let Some(v) = vin { record.vin = Some(v); }
        if let Some(v) = plate { record.plate = Some(v); }
        if let Some(v) = plate_state { record.plate_state = Some(v); }
        if let Some(v) = notes { record.notes = Some(v); }
        if let Some(v) = blob_ids { record.blob_ids = v; }
        record.updated_at = Utc::now();
        self.upsert_truck(&record).await?;
        Ok(record)
    }

    pub async fn update_truck_status(&self, id: Uuid, status: TruckStatus) -> Result<TruckRecord, AppError> {
        let mut record = self.get_truck_by_id(id).await?;
        record.status = status;
        record.updated_at = Utc::now();
        self.upsert_truck(&record).await?;
        Ok(record)
    }

    pub async fn update_truck_embedding(&self, id: Uuid, embedding: Vec<f32>) -> Result<(), AppError> {
        let mut record = self.get_truck_by_id(id).await?;
        record.embedding = Some(embedding);
        record.updated_at = Utc::now();
        self.upsert_truck(&record).await
    }

    pub async fn soft_delete_truck(&self, id: Uuid) -> Result<(), AppError> {
        let mut record = self.get_truck_by_id(id).await?;
        record.status = TruckStatus::Inactive;
        record.updated_at = Utc::now();
        self.upsert_truck(&record).await
    }

    pub async fn list_trucks(
        &self,
        status_filter: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(usize, Vec<TruckListItem>), AppError> {
        let filter = build_truck_filter(status_filter);
        let total = self.truck_table.count_rows(filter.clone()).await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let mut q = self.truck_table.query();
        if let Some(f) = filter { q = q.only_if(f); }
        let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let mut records = batches_to_trucks(collect_stream(stream).await?)?;
        records.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        let items: Vec<TruckListItem> = records.into_iter().skip(offset).take(limit).map(TruckListItem::from).collect();
        Ok((total, items))
    }

    pub async fn search_trucks(
        &self,
        embedding: Vec<f32>,
        status_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<TruckListItem>, AppError> {
        let filter = build_truck_filter(status_filter);
        let mut q = self.truck_table.query()
            .nearest_to(embedding)
            .map_err(|e| AppError::Internal(e.to_string()))?
            .limit(limit);
        if let Some(f) = filter { q = q.only_if(f); }
        let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let batches = collect_stream(stream).await?;
        let mut items = Vec::new();
        for batch in &batches {
            let distances = batch.column_by_name("_distance")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>())
                .map(|a| (0..a.len()).map(|i| a.value(i)).collect::<Vec<_>>());
            for (i, record) in batches_to_trucks(vec![batch.clone()])?.into_iter().enumerate() {
                let mut item = TruckListItem::from(record);
                if let Some(ref d) = distances { item.score = Some(1.0 / (1.0 + d[i])); }
                items.push(item);
            }
        }
        Ok(items)
    }

    pub async fn any_truck_references_blob(&self, blob_id: Uuid) -> Result<bool, AppError> {
        let id_str = blob_id.to_string();
        // Use JSON string boundaries to avoid false positives from UUID substrings
        let count = self.truck_table
            .count_rows(Some(format!("blob_ids LIKE '%\"{id_str}\"%'")))
            .await.map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(count > 0)
    }

    /// Ids of trucks that reference `blob_id` in their `blob_ids`.
    /// Used for the MCP `attached_to` reverse lookup.
    pub async fn trucks_referencing_blob(&self, blob_id: Uuid) -> Result<Vec<Uuid>, AppError> {
        let id_str = blob_id.to_string();
        let stream = self.truck_table.query()
            .only_if(format!("blob_ids LIKE '%\"{id_str}\"%'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(batches_to_trucks(collect_stream(stream).await?)?
            .into_iter().map(|r| r.id).collect())
    }

    pub async fn create_truck_vector_index(&self) -> Result<(), AppError> {
        self.create_ivfpq_index(&self.truck_table, "embedding", "trucks").await
    }
}

// --- Helpers ---

fn truck_to_batch(record: &TruckRecord, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let schema = truck_schema(embed_dim);
    let id_str = record.id.to_string();
    let created_str = record.created_at.to_rfc3339();
    let updated_str = record.updated_at.to_rfc3339();
    let blob_ids_json = serde_json::to_string(&record.blob_ids)
        .map_err(|e| AppError::Internal(e.to_string()))?;

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
        Arc::new(StringArray::from(vec![record.unit_number.as_str()])),
        Arc::new(Int64Array::from(vec![record.year.map(|y| y as i64)])),
        Arc::new(StringArray::from(vec![record.make.as_deref()])),
        Arc::new(StringArray::from(vec![record.model.as_deref()])),
        Arc::new(StringArray::from(vec![record.vin.as_deref()])),
        Arc::new(StringArray::from(vec![record.plate.as_deref()])),
        Arc::new(StringArray::from(vec![record.plate_state.as_deref()])),
        Arc::new(StringArray::from(vec![record.status.as_str()])),
        Arc::new(StringArray::from(vec![record.notes.as_deref()])),
        embedding_col,
        Arc::new(Int64Array::from(vec![record.owner_id])),
        Arc::new(StringArray::from(vec![created_str.as_str()])),
        Arc::new(StringArray::from(vec![updated_str.as_str()])),
        Arc::new(StringArray::from(vec![blob_ids_json.as_str()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_trucks(batches: Vec<RecordBatch>) -> Result<Vec<TruckRecord>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() { out.push(row_to_truck(batch, i)?); }
    }
    Ok(out)
}

fn row_to_truck(batch: &RecordBatch, i: usize) -> Result<TruckRecord, AppError> {
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
    let i64_col = |name: &str| -> i64 {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .map(|a| a.value(i)).unwrap_or(0)
    };
    let opt_i64 = |name: &str| -> Option<i64> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i)) })
    };

    let embedding = batch.column_by_name("embedding")
        .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
        .and_then(|fsl| {
            if fsl.is_null(i) { return None; }
            let values = fsl.value(i);
            values.as_any().downcast_ref::<Float32Array>()
                .map(|fa| (0..fa.len()).map(|j| fa.value(j)).collect::<Vec<f32>>())
        });

    Ok(TruckRecord {
        id: str_col("id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        unit_number: str_col("unit_number"),
        year: opt_i64("year").map(|y| y as i32),
        make: opt_str("make"),
        model: opt_str("model"),
        vin: opt_str("vin"),
        plate: opt_str("plate"),
        plate_state: opt_str("plate_state"),
        status: str_col("status").parse().map_err(|e: String| AppError::Internal(e))?,
        notes: opt_str("notes"),
        blob_ids: serde_json::from_str(&str_col("blob_ids")).unwrap_or_default(),
        embedding,
        owner_id: i64_col("owner_id"),
        created_at: str_col("created_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
        updated_at: str_col("updated_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
    })
}

fn build_truck_filter(status: Option<&str>) -> Option<String> {
    status.map(|s| format!("status = '{}'", s.replace('\'', "''")))
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

    fn sample_truck() -> TruckRecord {
        let now = chrono::Utc::now();
        TruckRecord {
            id: Uuid::new_v4(),
            unit_number: "T-101".into(),
            year: Some(2021),
            make: Some("Kenworth".into()),
            model: Some("T680".into()),
            vin: Some("1XKWD40X5MJ123456".into()),
            plate: Some("ABC1234".into()),
            plate_state: Some("TN".into()),
            status: TruckStatus::Available,
            notes: Some("primary flatbed unit".into()),
            blob_ids: vec![],
            embedding: None,
            owner_id: 0,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_insert_and_get_truck() {
        let (db, _dir) = test_db().await;
        let t = sample_truck();
        db.insert_truck(&t).await.unwrap();
        let fetched = db.get_truck_by_id(t.id).await.unwrap();
        assert_eq!(fetched.id, t.id);
        assert_eq!(fetched.unit_number, "T-101");
        assert_eq!(fetched.status, TruckStatus::Available);
        assert_eq!(fetched.year, Some(2021));
    }

    #[tokio::test]
    async fn test_get_truck_not_found() {
        let (db, _dir) = test_db().await;
        assert!(matches!(db.get_truck_by_id(Uuid::new_v4()).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_soft_delete_truck() {
        let (db, _dir) = test_db().await;
        let t = sample_truck();
        db.insert_truck(&t).await.unwrap();
        db.soft_delete_truck(t.id).await.unwrap();
        let fetched = db.get_truck_by_id(t.id).await.unwrap();
        assert_eq!(fetched.status, TruckStatus::Inactive);
    }

    #[tokio::test]
    async fn test_list_trucks_with_status_filter() {
        let (db, _dir) = test_db().await;
        let t = sample_truck();
        db.insert_truck(&t).await.unwrap();
        let (total, items) = db.list_trucks(Some("available"), 10, 0).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].id, t.id);
        let (total2, _) = db.list_trucks(Some("inactive"), 10, 0).await.unwrap();
        assert_eq!(total2, 0);
    }

    #[tokio::test]
    async fn test_update_truck_metadata() {
        let (db, _dir) = test_db().await;
        let t = sample_truck();
        db.insert_truck(&t).await.unwrap();
        let updated = db.update_truck_metadata(
            t.id,
            Some("T-202".into()),
            None, None, None, None, None, None, None, None,
        ).await.unwrap();
        assert_eq!(updated.unit_number, "T-202");
    }
}
