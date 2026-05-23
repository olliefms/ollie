// src/db/driver_ops.rs
use crate::{
    db::{driver_schema, DbClient},
    error::AppError,
    models::{DriverListItem, DriverRecord, DriverStatus},
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
    pub async fn insert_driver(&self, record: &DriverRecord) -> Result<(), AppError> {
        let batch = driver_to_batch(record, self.embed_dim)?;
        let schema = driver_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.driver_table.add(reader).execute().await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_driver_by_phone(&self, phone: &str) -> Result<Option<DriverRecord>, AppError> {
        let escaped = phone.replace('\'', "''");
        let stream = self.driver_table.query()
            .only_if(format!("phone = '{escaped}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let records = batches_to_drivers(collect_stream(stream).await?)?;
        Ok(records.into_iter().next())
    }

    pub async fn get_driver_by_id(&self, id: Uuid) -> Result<DriverRecord, AppError> {
        let id_str = id.to_string();
        let stream = self.driver_table.query()
            .only_if(format!("id = '{id_str}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        batches_to_drivers(collect_stream(stream).await?)?
            .into_iter().next()
            .ok_or(AppError::NotFound)
    }

    pub async fn batch_get_drivers(
        &self,
        ids: &[uuid::Uuid],
    ) -> Result<std::collections::HashMap<uuid::Uuid, crate::models::DriverRecord>, AppError> {
        if ids.is_empty() { return Ok(std::collections::HashMap::new()); }
        let id_list = ids.iter().map(|id| format!("'{id}'")).collect::<Vec<_>>().join(", ");
        let stream = self.driver_table.query()
            .only_if(format!("id IN ({id_list})"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(batches_to_drivers(collect_stream(stream).await?)?
            .into_iter().map(|r| (r.id, r)).collect())
    }

    async fn upsert_driver(&self, record: &DriverRecord) -> Result<(), AppError> {
        let batch = driver_to_batch(record, self.embed_dim)?;
        let schema = driver_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.driver_table.merge_insert(&["id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_driver_metadata(
        &self, id: Uuid,
        name: Option<String>,
        phone: Option<String>,
        email: Option<String>,
        license_number: Option<String>,
        license_state: Option<String>,
        license_expiry: Option<String>,
        notes: Option<String>,
    ) -> Result<DriverRecord, AppError> {
        let mut record = self.get_driver_by_id(id).await?;
        if let Some(v) = name { record.name = v; }
        if let Some(v) = phone { record.phone = Some(v); }
        if let Some(v) = email { record.email = Some(v); }
        if let Some(v) = license_number { record.license_number = Some(v); }
        if let Some(v) = license_state { record.license_state = Some(v); }
        if let Some(v) = license_expiry { record.license_expiry = Some(v); }
        if let Some(v) = notes { record.notes = Some(v); }
        record.updated_at = Utc::now();
        self.upsert_driver(&record).await?;
        Ok(record)
    }

    pub async fn update_driver_status(&self, id: Uuid, status: DriverStatus) -> Result<DriverRecord, AppError> {
        let mut record = self.get_driver_by_id(id).await?;
        record.status = status;
        record.updated_at = Utc::now();
        self.upsert_driver(&record).await?;
        Ok(record)
    }

    pub async fn update_driver_embedding(&self, id: Uuid, embedding: Vec<f32>) -> Result<(), AppError> {
        let mut record = self.get_driver_by_id(id).await?;
        record.embedding = Some(embedding);
        record.updated_at = Utc::now();
        self.upsert_driver(&record).await
    }

    pub async fn update_driver_equipment(
        &self,
        id: Uuid,
        current_truck_id: Option<Option<Uuid>>,
        current_trailer_ids: Option<Vec<Uuid>>,
    ) -> Result<DriverRecord, AppError> {
        let mut record = self.get_driver_by_id(id).await?;
        if let Some(truck) = current_truck_id { record.current_truck_id = truck; }
        if let Some(trailers) = current_trailer_ids { record.current_trailer_ids = trailers; }
        record.updated_at = Utc::now();
        self.upsert_driver(&record).await?;
        Ok(record)
    }

    pub async fn soft_delete_driver(&self, id: Uuid) -> Result<(), AppError> {
        let mut record = self.get_driver_by_id(id).await?;
        record.status = DriverStatus::Inactive;
        record.updated_at = Utc::now();
        self.upsert_driver(&record).await
    }

    pub async fn list_drivers(
        &self,
        status_filter: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(usize, Vec<DriverListItem>), AppError> {
        let filter = build_driver_filter(status_filter);
        let total = self.driver_table.count_rows(filter.clone()).await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let mut q = self.driver_table.query();
        if let Some(f) = filter { q = q.only_if(f); }
        let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let mut records = batches_to_drivers(collect_stream(stream).await?)?;
        records.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        let items: Vec<DriverListItem> = records.into_iter().skip(offset).take(limit).map(DriverListItem::from).collect();
        Ok((total, items))
    }

    pub async fn search_drivers(
        &self,
        embedding: Vec<f32>,
        status_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DriverListItem>, AppError> {
        let filter = build_driver_filter(status_filter);
        let mut q = self.driver_table.query()
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
            for (i, record) in batches_to_drivers(vec![batch.clone()])?.into_iter().enumerate() {
                let mut item = DriverListItem::from(record);
                if let Some(ref d) = distances { item.score = Some(1.0 / (1.0 + d[i])); }
                items.push(item);
            }
        }
        Ok(items)
    }

    pub async fn create_driver_vector_index(&self) -> Result<(), AppError> {
        self.driver_table
            .create_index(&["embedding"], lancedb::index::Index::IvfPq(Default::default()))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))
    }
}

// --- Helpers ---

fn driver_to_batch(record: &DriverRecord, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let schema = driver_schema(embed_dim);
    let id_str = record.id.to_string();
    let created_str = record.created_at.to_rfc3339();
    let updated_str = record.updated_at.to_rfc3339();
    let current_truck_str = record.current_truck_id.map(|u| u.to_string());
    let trailer_id_strs: Vec<String> = record.current_trailer_ids.iter().map(|u| u.to_string()).collect();
    let trailer_ids_json = serde_json::to_string(&trailer_id_strs)
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
        Arc::new(StringArray::from(vec![record.name.as_str()])),
        Arc::new(StringArray::from(vec![record.phone.as_deref()])),
        Arc::new(StringArray::from(vec![record.email.as_deref()])),
        Arc::new(StringArray::from(vec![record.license_number.as_deref()])),
        Arc::new(StringArray::from(vec![record.license_state.as_deref()])),
        Arc::new(StringArray::from(vec![record.license_expiry.as_deref()])),
        Arc::new(StringArray::from(vec![record.status.as_str()])),
        Arc::new(StringArray::from(vec![record.notes.as_deref()])),
        embedding_col,
        Arc::new(Int64Array::from(vec![record.owner_id])),
        Arc::new(StringArray::from(vec![created_str.as_str()])),
        Arc::new(StringArray::from(vec![updated_str.as_str()])),
        Arc::new(StringArray::from(vec![current_truck_str.as_deref()])),
        Arc::new(StringArray::from(vec![trailer_ids_json.as_str()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_drivers(batches: Vec<RecordBatch>) -> Result<Vec<DriverRecord>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() { out.push(row_to_driver(batch, i)?); }
    }
    Ok(out)
}

fn row_to_driver(batch: &RecordBatch, i: usize) -> Result<DriverRecord, AppError> {
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

    let embedding = batch.column_by_name("embedding")
        .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
        .and_then(|fsl| {
            if fsl.is_null(i) { return None; }
            let values = fsl.value(i);
            values.as_any().downcast_ref::<Float32Array>()
                .map(|fa| (0..fa.len()).map(|j| fa.value(j)).collect::<Vec<f32>>())
        });

    let current_truck_id = opt_str("current_truck_id")
        .map(|s| s.parse::<Uuid>())
        .transpose()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let current_trailer_ids = {
        let raw = opt_str("current_trailer_ids").unwrap_or_else(|| "[]".to_string());
        let strs: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
        strs.iter().filter_map(|s| s.parse::<Uuid>().ok()).collect()
    };

    Ok(DriverRecord {
        id: str_col("id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        name: str_col("name"),
        phone: opt_str("phone"),
        email: opt_str("email"),
        license_number: opt_str("license_number"),
        license_state: opt_str("license_state"),
        license_expiry: opt_str("license_expiry"),
        status: str_col("status").parse().map_err(|e: String| AppError::Internal(e))?,
        notes: opt_str("notes"),
        current_truck_id,
        current_trailer_ids,
        embedding,
        owner_id: i64_col("owner_id"),
        created_at: str_col("created_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
        updated_at: str_col("updated_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
    })
}

fn build_driver_filter(status: Option<&str>) -> Option<String> {
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

    fn sample_driver() -> DriverRecord {
        let now = chrono::Utc::now();
        DriverRecord {
            id: Uuid::new_v4(), name: "Alice Smith".into(),
            phone: Some("555-1234".into()), email: None,
            license_number: Some("CDL-12345".into()),
            license_state: Some("TN".into()),
            license_expiry: Some("2027-12-31".into()),
            status: DriverStatus::Available,
            notes: Some("experienced flatbed driver".into()),
            current_truck_id: None,
            current_trailer_ids: vec![],
            embedding: None, owner_id: 0,
            created_at: now, updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_insert_and_get_driver() {
        let (db, _dir) = test_db().await;
        let d = sample_driver();
        db.insert_driver(&d).await.unwrap();
        let fetched = db.get_driver_by_id(d.id).await.unwrap();
        assert_eq!(fetched.id, d.id);
        assert_eq!(fetched.name, "Alice Smith");
        assert_eq!(fetched.status, DriverStatus::Available);
    }

    #[tokio::test]
    async fn test_get_driver_not_found() {
        let (db, _dir) = test_db().await;
        assert!(matches!(db.get_driver_by_id(Uuid::new_v4()).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_soft_delete_driver() {
        let (db, _dir) = test_db().await;
        let d = sample_driver();
        db.insert_driver(&d).await.unwrap();
        db.soft_delete_driver(d.id).await.unwrap();
        let fetched = db.get_driver_by_id(d.id).await.unwrap();
        assert_eq!(fetched.status, DriverStatus::Inactive);
    }

    #[tokio::test]
    async fn test_list_drivers_with_status_filter() {
        let (db, _dir) = test_db().await;
        let d = sample_driver();
        db.insert_driver(&d).await.unwrap();
        let (total, items) = db.list_drivers(Some("available"), 10, 0).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].id, d.id);
        let (total2, _) = db.list_drivers(Some("inactive"), 10, 0).await.unwrap();
        assert_eq!(total2, 0);
    }

    #[tokio::test]
    async fn test_update_driver_metadata() {
        let (db, _dir) = test_db().await;
        let d = sample_driver();
        db.insert_driver(&d).await.unwrap();
        let updated = db.update_driver_metadata(
            d.id,
            Some("Bob Jones".into()),
            None, None, None, None, None, None,
        ).await.unwrap();
        assert_eq!(updated.name, "Bob Jones");
    }
}
