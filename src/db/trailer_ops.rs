// src/db/trailer_ops.rs
use crate::{
    db::{trailer_schema, DbClient},
    error::AppError,
    models::{TrailerListItem, TrailerOwner, TrailerRecord, TrailerStatus},
};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, Float64Array, Int64Array,
    RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray,
};
use chrono::Utc;
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;
use uuid::Uuid;

impl DbClient {
    pub async fn insert_trailer(&self, record: &TrailerRecord) -> Result<(), AppError> {
        let batch = trailer_to_batch(record, self.embed_dim)?;
        let schema = trailer_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.trailer_table.add(reader).execute().await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_trailer_by_id(&self, id: Uuid) -> Result<TrailerRecord, AppError> {
        let id_str = id.to_string();
        let stream = self.trailer_table.query()
            .only_if(format!("id = '{id_str}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        batches_to_trailers(collect_stream(stream).await?)?
            .into_iter().next()
            .ok_or(AppError::NotFound)
    }

    pub async fn batch_get_trailers(
        &self,
        ids: &[uuid::Uuid],
    ) -> Result<std::collections::HashMap<uuid::Uuid, crate::models::TrailerRecord>, AppError> {
        if ids.is_empty() { return Ok(std::collections::HashMap::new()); }
        let id_list = ids.iter().map(|id| format!("'{id}'")).collect::<Vec<_>>().join(", ");
        let stream = self.trailer_table.query()
            .only_if(format!("id IN ({id_list})"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(batches_to_trailers(collect_stream(stream).await?)?
            .into_iter().map(|r| (r.id, r)).collect())
    }

    async fn upsert_trailer(&self, record: &TrailerRecord) -> Result<(), AppError> {
        let batch = trailer_to_batch(record, self.embed_dim)?;
        let schema = trailer_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.trailer_table.merge_insert(&["id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_trailer_metadata(
        &self, id: Uuid,
        unit_number: Option<String>,
        owner: Option<TrailerOwner>,
        owner_name: Option<String>,
        year: Option<i32>,
        make: Option<String>,
        trailer_type: Option<String>,
        length_ft: Option<f64>,
        vin: Option<String>,
        plate: Option<String>,
        plate_state: Option<String>,
        notes: Option<String>,
    ) -> Result<TrailerRecord, AppError> {
        let mut record = self.get_trailer_by_id(id).await?;
        if let Some(v) = unit_number { record.unit_number = v; }
        if let Some(v) = owner { record.owner = v; }
        if let Some(v) = owner_name { record.owner_name = Some(v); }
        if let Some(v) = year { record.year = Some(v); }
        if let Some(v) = make { record.make = Some(v); }
        if let Some(v) = trailer_type { record.trailer_type = Some(v); }
        if let Some(v) = length_ft { record.length_ft = Some(v); }
        if let Some(v) = vin { record.vin = Some(v); }
        if let Some(v) = plate { record.plate = Some(v); }
        if let Some(v) = plate_state { record.plate_state = Some(v); }
        if let Some(v) = notes { record.notes = Some(v); }
        record.updated_at = Utc::now();
        self.upsert_trailer(&record).await?;
        Ok(record)
    }

    pub async fn update_trailer_status(&self, id: Uuid, status: TrailerStatus) -> Result<TrailerRecord, AppError> {
        let mut record = self.get_trailer_by_id(id).await?;
        record.status = status;
        record.updated_at = Utc::now();
        self.upsert_trailer(&record).await?;
        Ok(record)
    }

    pub async fn update_trailer_embedding(&self, id: Uuid, embedding: Vec<f32>) -> Result<(), AppError> {
        let mut record = self.get_trailer_by_id(id).await?;
        record.embedding = Some(embedding);
        record.updated_at = Utc::now();
        self.upsert_trailer(&record).await
    }

    pub async fn soft_delete_trailer(&self, id: Uuid) -> Result<(), AppError> {
        let mut record = self.get_trailer_by_id(id).await?;
        record.status = TrailerStatus::Inactive;
        record.updated_at = Utc::now();
        self.upsert_trailer(&record).await
    }

    pub async fn get_trailer_by_unit_number(&self, unit: &str) -> Result<Option<TrailerRecord>, AppError> {
        let escaped = unit.replace('\'', "''");
        let stream = self.trailer_table.query()
            .only_if(format!("unit_number = '{escaped}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let records = batches_to_trailers(collect_stream(stream).await?)?;
        Ok(records.into_iter().next())
    }

    pub async fn list_trailers(
        &self,
        status_filter: Option<&str>,
        owner_filter: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(usize, Vec<TrailerListItem>), AppError> {
        let filter = build_trailer_filter(status_filter, owner_filter);
        let total = self.trailer_table.count_rows(filter.clone()).await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let mut q = self.trailer_table.query();
        if let Some(f) = filter { q = q.only_if(f); }
        let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let mut records = batches_to_trailers(collect_stream(stream).await?)?;
        records.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        let items: Vec<TrailerListItem> = records.into_iter().skip(offset).take(limit).map(TrailerListItem::from).collect();
        Ok((total, items))
    }

    pub async fn search_trailers(
        &self,
        embedding: Vec<f32>,
        status_filter: Option<&str>,
        owner_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<TrailerListItem>, AppError> {
        let filter = build_trailer_filter(status_filter, owner_filter);
        let mut q = self.trailer_table.query()
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
            for (i, record) in batches_to_trailers(vec![batch.clone()])?.into_iter().enumerate() {
                let mut item = TrailerListItem::from(record);
                if let Some(ref d) = distances { item.score = Some(1.0 / (1.0 + d[i])); }
                items.push(item);
            }
        }
        Ok(items)
    }

    pub async fn create_trailer_vector_index(&self) -> Result<(), AppError> {
        self.trailer_table
            .create_index(&["embedding"], lancedb::index::Index::IvfPq(Default::default()))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))
    }
}

// --- Helpers ---

fn trailer_to_batch(record: &TrailerRecord, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let schema = trailer_schema(embed_dim);
    let id_str = record.id.to_string();
    let created_str = record.created_at.to_rfc3339();
    let updated_str = record.updated_at.to_rfc3339();
    let owner_str = record.owner.as_str();
    let status_str = record.status.as_str();

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
        Arc::new(StringArray::from(vec![owner_str])),
        Arc::new(StringArray::from(vec![record.owner_name.as_deref()])),
        Arc::new(Int64Array::from(vec![record.year.map(|y| y as i64)])),
        Arc::new(StringArray::from(vec![record.make.as_deref()])),
        Arc::new(StringArray::from(vec![record.trailer_type.as_deref()])),
        Arc::new(Float64Array::from(vec![record.length_ft])),
        Arc::new(StringArray::from(vec![record.vin.as_deref()])),
        Arc::new(StringArray::from(vec![record.plate.as_deref()])),
        Arc::new(StringArray::from(vec![record.plate_state.as_deref()])),
        Arc::new(StringArray::from(vec![status_str])),
        Arc::new(StringArray::from(vec![record.notes.as_deref()])),
        embedding_col,
        Arc::new(Int64Array::from(vec![record.owner_id])),
        Arc::new(StringArray::from(vec![created_str.as_str()])),
        Arc::new(StringArray::from(vec![updated_str.as_str()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_trailers(batches: Vec<RecordBatch>) -> Result<Vec<TrailerRecord>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() { out.push(row_to_trailer(batch, i)?); }
    }
    Ok(out)
}

fn row_to_trailer(batch: &RecordBatch, i: usize) -> Result<TrailerRecord, AppError> {
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
    let opt_f64 = |name: &str| -> Option<f64> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
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

    Ok(TrailerRecord {
        id: str_col("id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        unit_number: str_col("unit_number"),
        owner: str_col("owner").parse().map_err(|e: String| AppError::Internal(e))?,
        owner_name: opt_str("owner_name"),
        year: opt_i64("year").map(|y| y as i32),
        make: opt_str("make"),
        trailer_type: opt_str("trailer_type"),
        length_ft: opt_f64("length_ft"),
        vin: opt_str("vin"),
        plate: opt_str("plate"),
        plate_state: opt_str("plate_state"),
        status: str_col("status").parse().map_err(|e: String| AppError::Internal(e))?,
        notes: opt_str("notes"),
        embedding,
        owner_id: i64_col("owner_id"),
        created_at: str_col("created_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
        updated_at: str_col("updated_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
    })
}

fn build_trailer_filter(status: Option<&str>, owner: Option<&str>) -> Option<String> {
    match (status, owner) {
        (Some(s), Some(o)) => Some(format!(
            "status = '{}' AND owner = '{}'",
            s.replace('\'', "''"),
            o.replace('\'', "''")
        )),
        (Some(s), None) => Some(format!("status = '{}'", s.replace('\'', "''"))),
        (None, Some(o)) => Some(format!("owner = '{}'", o.replace('\'', "''"))),
        (None, None) => None,
    }
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

    fn sample_trailer() -> TrailerRecord {
        let now = chrono::Utc::now();
        TrailerRecord {
            id: Uuid::new_v4(),
            unit_number: "TR-101".into(),
            owner: TrailerOwner::Fleet,
            owner_name: None,
            year: Some(2021),
            make: Some("Wabash".into()),
            trailer_type: Some("dry_van".into()),
            length_ft: Some(53.0),
            vin: Some("1ABCD40X5MJ123456".into()),
            plate: Some("XYZ9876".into()),
            plate_state: Some("TN".into()),
            status: TrailerStatus::Available,
            notes: Some("primary dry van".into()),
            embedding: None,
            owner_id: 0,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_insert_and_get_trailer() {
        let (db, _dir) = test_db().await;
        let t = sample_trailer();
        db.insert_trailer(&t).await.unwrap();
        let fetched = db.get_trailer_by_id(t.id).await.unwrap();
        assert_eq!(fetched.id, t.id);
        assert_eq!(fetched.unit_number, "TR-101");
        assert_eq!(fetched.status, TrailerStatus::Available);
        assert_eq!(fetched.year, Some(2021));
        assert_eq!(fetched.length_ft, Some(53.0));
    }

    #[tokio::test]
    async fn test_get_trailer_not_found() {
        let (db, _dir) = test_db().await;
        assert!(matches!(db.get_trailer_by_id(Uuid::new_v4()).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_soft_delete_trailer() {
        let (db, _dir) = test_db().await;
        let t = sample_trailer();
        db.insert_trailer(&t).await.unwrap();
        db.soft_delete_trailer(t.id).await.unwrap();
        let fetched = db.get_trailer_by_id(t.id).await.unwrap();
        assert_eq!(fetched.status, TrailerStatus::Inactive);
    }

    #[tokio::test]
    async fn test_list_trailers_with_status_filter() {
        let (db, _dir) = test_db().await;
        let t = sample_trailer();
        db.insert_trailer(&t).await.unwrap();
        let (total, items) = db.list_trailers(Some("available"), None, 10, 0).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].id, t.id);
        let (total2, _) = db.list_trailers(Some("inactive"), None, 10, 0).await.unwrap();
        assert_eq!(total2, 0);
    }

    #[tokio::test]
    async fn test_list_trailers_with_owner_filter() {
        let (db, _dir) = test_db().await;
        let t = sample_trailer();
        db.insert_trailer(&t).await.unwrap();
        let (total, _) = db.list_trailers(None, Some("fleet"), 10, 0).await.unwrap();
        assert_eq!(total, 1);
        let (total2, _) = db.list_trailers(None, Some("carrier"), 10, 0).await.unwrap();
        assert_eq!(total2, 0);
    }

    #[tokio::test]
    async fn test_batch_get_trailers() {
        let (db, _dir) = test_db().await;
        let t1 = sample_trailer();
        let t2 = {
            let mut t = sample_trailer();
            t.id = uuid::Uuid::new_v4();
            t.unit_number = "TRL-002".into();
            t
        };
        db.insert_trailer(&t1).await.unwrap();
        db.insert_trailer(&t2).await.unwrap();

        let map = db.batch_get_trailers(&[t1.id, t2.id]).await.unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map[&t1.id].unit_number, t1.unit_number);
        assert_eq!(map[&t2.id].unit_number, "TRL-002");

        let empty = db.batch_get_trailers(&[]).await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_update_trailer_metadata() {
        let (db, _dir) = test_db().await;
        let t = sample_trailer();
        db.insert_trailer(&t).await.unwrap();
        let updated = db.update_trailer_metadata(
            t.id,
            Some("TR-202".into()),
            None, None, None, None, None, None, None, None, None, None,
        ).await.unwrap();
        assert_eq!(updated.unit_number, "TR-202");
    }
}
