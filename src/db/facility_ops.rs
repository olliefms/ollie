// src/db/facility_ops.rs
use crate::{
    db::{facility_schema, DbClient},
    error::AppError,
    models::{FacilityListItem, FacilityRecord, GeocodeStatus},
};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, Float64Array, Int64Array,
    RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray,
};
use chrono::Utc;
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::{collections::HashMap, sync::Arc};
use uuid::Uuid;

impl DbClient {
    pub async fn insert_facility(&self, record: &FacilityRecord) -> Result<(), AppError> {
        let batch = facility_to_batch(record, self.embed_dim)?;
        let schema = facility_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.facility_table.add(reader).execute().await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_facility_by_id(&self, id: Uuid) -> Result<FacilityRecord, AppError> {
        let stream = self.facility_table.query()
            .only_if(format!("id = '{}'", id))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let batches = collect_stream(stream).await?;
        batches_to_facilities(batches)?
            .into_iter().next()
            .ok_or(AppError::NotFound)
    }

    pub async fn delete_facility_by_id(&self, id: Uuid) -> Result<(), AppError> {
        self.facility_table.delete(&format!("id = '{id}'")).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_facility_metadata(
        &self, id: Uuid,
        name: Option<String>, address: Option<String>,
        contacts: Option<Vec<crate::models::FacilityContact>>,
        notes: Option<String>, tags: Option<Vec<String>>,
        blob_ids: Option<Vec<Uuid>>,
    ) -> Result<FacilityRecord, AppError> {
        let mut record = self.get_facility_by_id(id).await?;
        if let Some(n) = name { record.name = n; }
        if let Some(a) = address {
            record.address = a;
            record.normalized_address = None;
            record.lat = None;
            record.lng = None;
            record.geocode_status = GeocodeStatus::Pending;
        }
        if let Some(c) = contacts { record.contacts = c; }
        if let Some(n) = notes { record.notes = Some(n); }
        if let Some(t) = tags { record.tags = t; }
        if let Some(b) = blob_ids { record.blob_ids = b; }
        record.updated_at = Utc::now();
        self.upsert_facility(&record).await?;
        Ok(record)
    }

    pub async fn update_facility_geocode(
        &self, id: Uuid, lat: f64, lng: f64, normalized_address: String,
    ) -> Result<(), AppError> {
        let mut record = self.get_facility_by_id(id).await?;
        record.lat = Some(lat);
        record.lng = Some(lng);
        record.normalized_address = Some(normalized_address);
        record.geocode_status = GeocodeStatus::Ready;
        record.updated_at = Utc::now();
        self.upsert_facility(&record).await
    }

    pub async fn mark_facility_geocode_failed(&self, id: Uuid) -> Result<(), AppError> {
        let mut record = self.get_facility_by_id(id).await?;
        record.geocode_status = GeocodeStatus::Failed;
        record.updated_at = Utc::now();
        self.upsert_facility(&record).await
    }

    pub async fn update_facility_embedding(
        &self, id: Uuid, embedding: Vec<f32>,
    ) -> Result<(), AppError> {
        let mut record = self.get_facility_by_id(id).await?;
        record.embedding = Some(embedding);
        record.updated_at = Utc::now();
        self.upsert_facility(&record).await
    }

    async fn upsert_facility(&self, record: &FacilityRecord) -> Result<(), AppError> {
        let batch = facility_to_batch(record, self.embed_dim)?;
        let schema = facility_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.facility_table.merge_insert(&["id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn list_facilities(
        &self,
        name_filter: Option<&str>,
        tag_filter: &[String],
        limit: usize,
        offset: usize,
    ) -> Result<(usize, Vec<FacilityListItem>), AppError> {
        let filter = build_facility_filter(name_filter, tag_filter);
        let total = self.facility_table.count_rows(filter.clone()).await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let mut q = self.facility_table.query().limit(limit + offset);
        if let Some(f) = filter { q = q.only_if(f); }
        let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let items: Vec<FacilityListItem> = batches_to_facilities(collect_stream(stream).await?)?
            .into_iter().skip(offset).map(FacilityListItem::from).collect();
        Ok((total, items))
    }

    pub async fn search_facilities(
        &self,
        embedding: Vec<f32>,
        name_filter: Option<&str>,
        tag_filter: &[String],
        limit: usize,
    ) -> Result<Vec<FacilityListItem>, AppError> {
        let filter = build_facility_filter(name_filter, tag_filter);
        let mut q = self.facility_table.query()
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
            for (i, record) in batches_to_facilities(vec![batch.clone()])?.into_iter().enumerate() {
                let mut item = FacilityListItem::from(record);
                if let Some(ref d) = distances {
                    item.score = Some(1.0 / (1.0 + d[i]));
                }
                items.push(item);
            }
        }
        Ok(items)
    }

    pub async fn batch_get_facilities(
        &self,
        ids: &[Uuid],
    ) -> Result<HashMap<Uuid, FacilityRecord>, AppError> {
        if ids.is_empty() { return Ok(HashMap::new()); }
        let id_list = ids.iter().map(|id| format!("'{id}'")).collect::<Vec<_>>().join(", ");
        let stream = self.facility_table.query()
            .only_if(format!("id IN ({id_list})"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(batches_to_facilities(collect_stream(stream).await?)?
            .into_iter().map(|r| (r.id, r)).collect())
    }

    pub async fn list_pending_geocode_facility_ids(&self) -> Result<Vec<Uuid>, AppError> {
        let stream = self.facility_table.query()
            .only_if("geocode_status = 'pending'")
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(batches_to_facilities(collect_stream(stream).await?)?
            .into_iter().map(|r| r.id).collect())
    }

    pub async fn create_facility_vector_index(&self) -> Result<(), AppError> {
        self.facility_table
            .create_index(&["embedding"], lancedb::index::Index::IvfPq(Default::default()))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))
    }
}

// --- Helpers ---

fn facility_to_batch(record: &FacilityRecord, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let schema = facility_schema(embed_dim);
    let contacts_json = serde_json::to_string(&record.contacts)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let tags_json = serde_json::to_string(&record.tags)
        .map_err(|e| AppError::Internal(e.to_string()))?;
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
        Arc::new(StringArray::from(vec![record.id.to_string().as_str()])),
        Arc::new(Int64Array::from(vec![record.owner_id])),
        Arc::new(StringArray::from(vec![record.name.as_str()])),
        Arc::new(StringArray::from(vec![record.address.as_str()])),
        Arc::new(StringArray::from(vec![record.normalized_address.as_deref()])),
        Arc::new(Float64Array::from(vec![record.lat])),
        Arc::new(Float64Array::from(vec![record.lng])),
        Arc::new(StringArray::from(vec![record.geocode_status.as_str()])),
        Arc::new(StringArray::from(vec![contacts_json.as_str()])),
        Arc::new(StringArray::from(vec![record.notes.as_deref()])),
        Arc::new(StringArray::from(vec![tags_json.as_str()])),
        Arc::new(StringArray::from(vec![blob_ids_json.as_str()])),
        Arc::new(Float64Array::from(vec![record.avg_dwell_minutes])),
        Arc::new(Int64Array::from(vec![record.dwell_sample_count])),
        embedding_col,
        Arc::new(StringArray::from(vec![record.created_at.to_rfc3339().as_str()])),
        Arc::new(StringArray::from(vec![record.updated_at.to_rfc3339().as_str()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_facilities(batches: Vec<RecordBatch>) -> Result<Vec<FacilityRecord>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() {
            out.push(row_to_facility(batch, i)?);
        }
    }
    Ok(out)
}

fn row_to_facility(batch: &RecordBatch, i: usize) -> Result<FacilityRecord, AppError> {
    let str_col = |name: &str| -> String {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .map(|a| a.value(i).to_string())
            .unwrap_or_default()
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
    let opt_f64 = |name: &str| -> Option<f64> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i)) })
    };

    let contacts: Vec<crate::models::FacilityContact> =
        serde_json::from_str(&str_col("contacts")).unwrap_or_default();
    let tags: Vec<String> =
        serde_json::from_str(&str_col("tags")).unwrap_or_default();
    let blob_ids: Vec<Uuid> =
        serde_json::from_str(&str_col("blob_ids")).unwrap_or_default();

    let embedding = batch.column_by_name("embedding")
        .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
        .and_then(|fsl| {
            if fsl.is_null(i) { return None; }
            let values = fsl.value(i);
            values.as_any().downcast_ref::<Float32Array>()
                .map(|fa| (0..fa.len()).map(|j| fa.value(j)).collect::<Vec<f32>>())
        });

    Ok(FacilityRecord {
        id: str_col("id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        owner_id: i64_col("owner_id"),
        name: str_col("name"),
        address: str_col("address"),
        normalized_address: opt_str("normalized_address"),
        lat: opt_f64("lat"),
        lng: opt_f64("lng"),
        geocode_status: str_col("geocode_status").parse()
            .map_err(|e: String| AppError::Internal(e))?,
        contacts, notes: opt_str("notes"), tags, blob_ids,
        avg_dwell_minutes: opt_f64("avg_dwell_minutes"),
        dwell_sample_count: i64_col("dwell_sample_count"),
        embedding,
        created_at: str_col("created_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
        updated_at: str_col("updated_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
    })
}

fn build_facility_filter(name: Option<&str>, tags: &[String]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    // Escape single quotes to prevent SQL injection in LanceDB filter strings
    if let Some(n) = name {
        let n = n.replace('\'', "''");
        parts.push(format!("name LIKE '%{n}%'"));
    }
    for tag in tags {
        let tag = tag.replace('\'', "''");
        parts.push(format!("tags LIKE '%\"{tag}\"%'"));
    }
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
    use crate::models::GeocodeStatus;
    use tempfile::TempDir;

    async fn test_db() -> (DbClient, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        (db, dir)
    }

    fn sample_facility() -> FacilityRecord {
        let now = chrono::Utc::now();
        FacilityRecord {
            id: uuid::Uuid::new_v4(), owner_id: 0,
            name: "ABC Warehouse".into(), address: "Memphis, TN".into(),
            normalized_address: None, lat: None, lng: None,
            geocode_status: GeocodeStatus::Pending,
            contacts: vec![], notes: None, tags: vec!["cold".into()],
            blob_ids: vec![], avg_dwell_minutes: None, dwell_sample_count: 0,
            embedding: None, created_at: now, updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_insert_and_get_facility() {
        let (db, _dir) = test_db().await;
        let f = sample_facility();
        db.insert_facility(&f).await.unwrap();
        let fetched = db.get_facility_by_id(f.id).await.unwrap();
        assert_eq!(fetched.id, f.id);
        assert_eq!(fetched.name, "ABC Warehouse");
        assert_eq!(fetched.tags, vec!["cold"]);
    }

    #[tokio::test]
    async fn test_get_facility_not_found() {
        let (db, _dir) = test_db().await;
        assert!(matches!(db.get_facility_by_id(uuid::Uuid::new_v4()).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_delete_facility() {
        let (db, _dir) = test_db().await;
        let f = sample_facility();
        db.insert_facility(&f).await.unwrap();
        db.delete_facility_by_id(f.id).await.unwrap();
        assert!(matches!(db.get_facility_by_id(f.id).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_update_facility_geocode() {
        let (db, _dir) = test_db().await;
        let f = sample_facility();
        db.insert_facility(&f).await.unwrap();
        db.update_facility_geocode(f.id, 35.1495, -90.0490, "315 Industrial Blvd, Memphis, TN 38118".into()).await.unwrap();
        let fetched = db.get_facility_by_id(f.id).await.unwrap();
        assert_eq!(fetched.geocode_status, GeocodeStatus::Ready);
        assert!((fetched.lat.unwrap() - 35.1495).abs() < 1e-6);
        assert!(fetched.normalized_address.is_some());
    }

    #[tokio::test]
    async fn test_list_facilities_with_tag_filter() {
        let (db, _dir) = test_db().await;
        let f = sample_facility();
        db.insert_facility(&f).await.unwrap();
        let (total, items) = db.list_facilities(None, &["cold".to_string()], 10, 0).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].id, f.id);
    }

    #[tokio::test]
    async fn test_batch_get_facilities() {
        let (db, _dir) = test_db().await;
        let f1 = sample_facility();
        let mut f2 = sample_facility();
        f2.id = uuid::Uuid::new_v4();
        f2.name = "XYZ Dock".into();
        db.insert_facility(&f1).await.unwrap();
        db.insert_facility(&f2).await.unwrap();
        let map = db.batch_get_facilities(&[f1.id, f2.id]).await.unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map[&f1.id].name, "ABC Warehouse");
        assert_eq!(map[&f2.id].name, "XYZ Dock");
    }
}
