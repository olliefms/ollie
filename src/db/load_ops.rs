// src/db/load_ops.rs

/// Maximum rows fetched from LanceDB in a single `list_loads` scan.
/// LanceDB 0.27 has no ORDER BY, so all matching rows are fetched into memory,
/// sorted by `created_at DESC`, then paginated with `skip/take`.
///
/// **Pagination divergence:** `count_rows` returns the total number of matching rows
/// in the table (unbounded), while this cap limits how many rows are actually fetched.
/// Any offset >= LOAD_SCAN_CAP will return an empty page even though `returned` in
/// the API response still reflects the full count. If load volume grows past ~2 000
/// filtered records, raise this constant or switch to cursor-based pagination.
const LOAD_SCAN_CAP: usize = 2_000;

use crate::{
    db::{load_schema, DbClient},
    error::AppError,
    models::{LoadListItem, LoadRecord, LoadStatus, Stop},
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
    pub async fn insert_load(&self, record: &LoadRecord) -> Result<(), AppError> {
        let batch = load_to_batch(record, self.embed_dim)?;
        let schema = load_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.load_table.add(reader).execute().await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_load_by_id(&self, id: Uuid) -> Result<LoadRecord, AppError> {
        let stream = self.load_table.query()
            .only_if(format!("id = '{id}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        batches_to_loads(collect_stream(stream).await?)?
            .into_iter().next()
            .ok_or(AppError::NotFound)
    }

    pub async fn get_load_by_number(&self, load_number: &str) -> Result<LoadRecord, AppError> {
        let escaped = load_number.replace('\'', "''");
        let stream = self.load_table.query()
            .only_if(format!("load_number = '{escaped}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        batches_to_loads(collect_stream(stream).await?)?
            .into_iter().next()
            .ok_or(AppError::NotFound)
    }

    pub async fn delete_load_by_id(&self, id: Uuid) -> Result<(), AppError> {
        self.load_table.delete(&format!("id = '{id}'")).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_load_metadata(
        &self, id: Uuid,
        customer_name: Option<String>, customer_ref: Option<String>,
        stops: Option<Vec<Stop>>,
        rate_items: Option<Vec<crate::models::RateLineItem>>,
        commodity: Option<String>, weight_lbs: Option<f64>, miles: Option<f64>,
        notes: Option<String>, tags: Option<Vec<String>>, blob_ids: Option<Vec<Uuid>>,
        embedding: Option<Vec<f32>>,
    ) -> Result<LoadRecord, AppError> {
        let mut record = self.get_load_by_id(id).await?;
        if let Some(v) = customer_name { record.customer_name = v; }
        if let Some(v) = customer_ref { record.customer_ref = Some(v); }
        if let Some(v) = stops { record.stops = v; }
        if let Some(v) = rate_items { record.rate_items = v; }
        if let Some(v) = commodity { record.commodity = Some(v); }
        if let Some(v) = weight_lbs { record.weight_lbs = Some(v); }
        if let Some(v) = miles { record.miles = Some(v); }
        if let Some(v) = notes { record.notes = Some(v); }
        if let Some(v) = tags { record.tags = v; }
        if let Some(v) = blob_ids { record.blob_ids = v; }
        if let Some(v) = embedding { record.embedding = Some(v); }
        record.updated_at = Utc::now();
        self.upsert_load(&record).await?;
        Ok(record)
    }

    pub async fn transition_load_status(
        &self, id: Uuid, new_status: LoadStatus,
        invoice_number: Option<String>,
        invoice_date: Option<String>,
        cancellation_reason: Option<String>,
    ) -> Result<LoadRecord, AppError> {
        let mut record = self.get_load_by_id(id).await?;
        if !record.status.can_transition_to(&new_status) {
            return Err(AppError::Conflict(format!(
                "cannot transition from '{}' to '{}'",
                record.status.as_str(), new_status.as_str()
            )));
        }
        record.status = new_status;
        if let Some(v) = invoice_number { record.invoice_number = Some(v); }
        if let Some(v) = invoice_date { record.invoice_date = Some(v); }
        if let Some(v) = cancellation_reason { record.cancellation_reason = Some(v); }
        record.updated_at = Utc::now();
        self.upsert_load(&record).await?;
        Ok(record)
    }

    pub async fn update_load_miles(&self, id: Uuid, miles: f64) -> Result<(), AppError> {
        let mut record = self.get_load_by_id(id).await?;
        record.miles = Some(miles);
        record.updated_at = Utc::now();
        self.upsert_load(&record).await
    }

    pub async fn clear_load_miles(&self, id: Uuid) -> Result<(), AppError> {
        let mut record = self.get_load_by_id(id).await?;
        record.miles = None;
        record.updated_at = Utc::now();
        self.upsert_load(&record).await
    }

    async fn upsert_load(&self, record: &LoadRecord) -> Result<(), AppError> {
        let batch = load_to_batch(record, self.embed_dim)?;
        let schema = load_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.load_table.merge_insert(&["id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_loads(
        &self,
        status_filter: Option<&str>,
        customer_filter: Option<&str>,
        tag_filter: &[String],
        from_date: Option<&str>,
        to_date: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(usize, Vec<LoadListItem>), AppError> {
        let filter = build_load_filter(status_filter, customer_filter, tag_filter, from_date, to_date)?;
        let total = self.load_table.count_rows(filter.clone()).await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let mut q = self.load_table.query().limit(LOAD_SCAN_CAP);
        if let Some(f) = filter { q = q.only_if(f); }
        let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let mut records = batches_to_loads(collect_stream(stream).await?)?;
        records.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        let items: Vec<LoadListItem> = records.into_iter().skip(offset).take(limit).map(LoadListItem::from).collect();
        Ok((total, items))
    }

    pub async fn search_loads(
        &self,
        embedding: Vec<f32>,
        status_filter: Option<&str>,
        customer_filter: Option<&str>,
        tag_filter: &[String],
        limit: usize,
    ) -> Result<Vec<LoadListItem>, AppError> {
        let filter = build_load_filter(status_filter, customer_filter, tag_filter, None, None)?;
        let mut q = self.load_table.query()
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
            for (i, record) in batches_to_loads(vec![batch.clone()])?.into_iter().enumerate() {
                let mut item = LoadListItem::from(record);
                if let Some(ref d) = distances { item.score = Some(1.0 / (1.0 + d[i])); }
                items.push(item);
            }
        }
        Ok(items)
    }

    pub async fn next_load_number(&self, year: i32) -> Result<String, AppError> {
        let prefix = format!("LD-{year}-");
        let stream = self.load_table.query()
            .only_if(format!("load_number LIKE '{prefix}%'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let records = batches_to_loads(collect_stream(stream).await?)?;
        let max_n = records.iter()
            .filter_map(|r| r.load_number.strip_prefix(&prefix))
            .filter_map(|s| s.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        Ok(format!("{prefix}{:04}", max_n + 1))
    }

    pub async fn any_load_references_facility(&self, facility_id: Uuid) -> Result<bool, AppError> {
        // Use JSON string boundaries ("%"uuid"%) to avoid false positives from UUID substrings
        let count = self.load_table
            .count_rows(Some(format!("stops LIKE '%\"{}\"%'", facility_id)))
            .await.map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(count > 0)
    }

    pub async fn any_load_references_blob(&self, blob_id: Uuid) -> Result<bool, AppError> {
        let id_str = blob_id.to_string();
        // Use JSON string boundaries to avoid false positives from UUID substrings
        let blob_count = self.load_table
            .count_rows(Some(format!("blob_ids LIKE '%\"{id_str}\"%'")))
            .await.map_err(|e| AppError::Internal(e.to_string()))?;
        let stop_count = self.load_table
            .count_rows(Some(format!("stops LIKE '%\"{id_str}\"%'")))
            .await.map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(blob_count + stop_count > 0)
    }

    /// Ids of loads that reference `blob_id` (in the load's `blob_ids` or any
    /// stop's `blob_ids`). Used for the MCP `attached_to` reverse lookup.
    pub async fn loads_referencing_blob(&self, blob_id: Uuid) -> Result<Vec<Uuid>, AppError> {
        let id_str = blob_id.to_string();
        let stream = self.load_table.query()
            .only_if(format!("blob_ids LIKE '%\"{id_str}\"%' OR stops LIKE '%\"{id_str}\"%'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(batches_to_loads(collect_stream(stream).await?)?
            .into_iter().map(|r| r.id).collect())
    }

    pub async fn list_loads_needing_routing(&self) -> Result<Vec<Uuid>, AppError> {
        // loads with no miles and non-terminal status
        let stream = self.load_table.query()
            .only_if("miles IS NULL AND status NOT IN ('delivered','invoiced','settled','cancelled')")
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(batches_to_loads(collect_stream(stream).await?)?
            .into_iter().map(|r| r.id).collect())
    }

    pub async fn list_unrouted_loads_for_facility(&self, facility_id: Uuid) -> Result<Vec<Uuid>, AppError> {
        let fac_str = facility_id.to_string();
        let filter = format!(
            "miles IS NULL AND status NOT IN ('delivered','invoiced','settled','cancelled') AND stops LIKE '%\"{}\"%'",
            fac_str
        );
        let stream = self.load_table.query()
            .only_if(filter)
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(batches_to_loads(collect_stream(stream).await?)?
            .into_iter().map(|r| r.id).collect())
    }

    pub async fn create_load_vector_index(&self) -> Result<(), AppError> {
        self.create_ivfpq_index(&self.load_table, "embedding", "loads").await
    }
}

// --- Helpers ---

fn load_to_batch(record: &LoadRecord, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let schema = load_schema(embed_dim);
    let stops_json = serde_json::to_string(&record.stops)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let rate_items_json = serde_json::to_string(&record.rate_items)
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
        Arc::new(StringArray::from(vec![record.load_number.as_str()])),
        Arc::new(Int64Array::from(vec![record.owner_id])),
        Arc::new(StringArray::from(vec![record.status.as_str()])),
        Arc::new(StringArray::from(vec![record.customer_name.as_str()])),
        Arc::new(StringArray::from(vec![record.customer_ref.as_deref()])),
        Arc::new(StringArray::from(vec![stops_json.as_str()])),
        Arc::new(StringArray::from(vec![rate_items_json.as_str()])),
        Arc::new(StringArray::from(vec![record.commodity.as_deref()])),
        Arc::new(Float64Array::from(vec![record.weight_lbs])),
        Arc::new(Float64Array::from(vec![record.miles])),
        Arc::new(StringArray::from(vec![record.notes.as_deref()])),
        Arc::new(StringArray::from(vec![tags_json.as_str()])),
        Arc::new(StringArray::from(vec![blob_ids_json.as_str()])),
        Arc::new(StringArray::from(vec![record.invoice_number.as_deref()])),
        Arc::new(StringArray::from(vec![record.invoice_date.as_deref()])),
        Arc::new(StringArray::from(vec![record.cancellation_reason.as_deref()])),
        embedding_col,
        Arc::new(StringArray::from(vec![record.created_at.to_rfc3339().as_str()])),
        Arc::new(StringArray::from(vec![record.updated_at.to_rfc3339().as_str()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_loads(batches: Vec<RecordBatch>) -> Result<Vec<LoadRecord>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() { out.push(row_to_load(batch, i)?); }
    }
    Ok(out)
}

fn row_to_load(batch: &RecordBatch, i: usize) -> Result<LoadRecord, AppError> {
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
    let opt_f64 = |name: &str| -> Option<f64> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i)) })
    };

    let stops: Vec<Stop> = serde_json::from_str(&str_col("stops")).unwrap_or_default();
    let rate_items: Vec<crate::models::RateLineItem> =
        serde_json::from_str(&str_col("rate_items")).unwrap_or_default();
    let tags: Vec<String> = serde_json::from_str(&str_col("tags")).unwrap_or_default();
    let blob_ids: Vec<Uuid> = serde_json::from_str(&str_col("blob_ids")).unwrap_or_default();

    let embedding = batch.column_by_name("embedding")
        .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
        .and_then(|fsl| {
            if fsl.is_null(i) { return None; }
            let values = fsl.value(i);
            values.as_any().downcast_ref::<Float32Array>()
                .map(|fa| (0..fa.len()).map(|j| fa.value(j)).collect::<Vec<f32>>())
        });

    Ok(LoadRecord {
        id: str_col("id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        load_number: str_col("load_number"), owner_id: i64_col("owner_id"),
        status: str_col("status").parse().map_err(|e: String| AppError::Internal(e))?,
        customer_name: str_col("customer_name"), customer_ref: opt_str("customer_ref"),
        stops, rate_items,
        commodity: opt_str("commodity"), weight_lbs: opt_f64("weight_lbs"),
        miles: opt_f64("miles"), notes: opt_str("notes"), tags, blob_ids,
        invoice_number: opt_str("invoice_number"), invoice_date: opt_str("invoice_date"),
        cancellation_reason: opt_str("cancellation_reason"),
        embedding,
        created_at: str_col("created_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
        updated_at: str_col("updated_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
    })
}

fn build_load_filter(
    status: Option<&str>, customer: Option<&str>,
    tags: &[String], from: Option<&str>, to: Option<&str>,
) -> Result<Option<String>, AppError> {
    let mut parts: Vec<String> = Vec::new();
    // Escape single quotes to prevent SQL injection in LanceDB filter strings
    if let Some(s) = status { parts.push(format!("status = '{}'", s.replace('\'', "''"))); }
    if let Some(c) = customer {
        let c = c.replace('\'', "''");
        parts.push(format!("customer_name LIKE '%{c}%'"));
    }
    for tag in tags {
        let tag = tag.replace('\'', "''");
        parts.push(format!("tags LIKE '%\"{tag}\"%'"));
    }
    if let Some(f) = from {
        chrono::DateTime::parse_from_rfc3339(f)
            .map_err(|_| AppError::BadRequest("invalid 'from' datetime".into()))?;
        parts.push(format!("created_at >= '{f}'"));
    }
    if let Some(t) = to {
        chrono::DateTime::parse_from_rfc3339(t)
            .map_err(|_| AppError::BadRequest("invalid 'to' datetime".into()))?;
        parts.push(format!("created_at <= '{t}'"));
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
    use crate::models::{LoadStatus, RateLineItem};
    use tempfile::TempDir;

    async fn test_db() -> (DbClient, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        (db, dir)
    }

    fn sample_load() -> LoadRecord {
        let now = chrono::Utc::now();
        LoadRecord {
            id: uuid::Uuid::new_v4(),
            load_number: "LD-2026-0001".into(),
            owner_id: 0, status: LoadStatus::Planned,
            customer_name: "ACME Logistics".into(), customer_ref: None,
            stops: vec![], rate_items: vec![
                RateLineItem { description: "Line Haul".into(), amount_usd: 1500.0 },
            ],
            commodity: Some("dry goods".into()), weight_lbs: Some(40000.0),
            miles: None, notes: None, tags: vec!["flatbed".into()],
            blob_ids: vec![], invoice_number: None, invoice_date: None,
            cancellation_reason: None, embedding: None,
            created_at: now, updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_insert_and_get_load() {
        let (db, _dir) = test_db().await;
        let load = sample_load();
        db.insert_load(&load).await.unwrap();
        let fetched = db.get_load_by_id(load.id).await.unwrap();
        assert_eq!(fetched.id, load.id);
        assert_eq!(fetched.customer_name, "ACME Logistics");
        assert_eq!(fetched.rate_items.len(), 1);
    }

    #[tokio::test]
    async fn test_get_load_not_found() {
        let (db, _dir) = test_db().await;
        assert!(matches!(db.get_load_by_id(uuid::Uuid::new_v4()).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_delete_load() {
        let (db, _dir) = test_db().await;
        let load = sample_load();
        db.insert_load(&load).await.unwrap();
        db.delete_load_by_id(load.id).await.unwrap();
        assert!(matches!(db.get_load_by_id(load.id).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_transition_status() {
        let (db, _dir) = test_db().await;
        let load = sample_load();
        db.insert_load(&load).await.unwrap();
        db.transition_load_status(load.id, LoadStatus::Assigned, None, None, None).await.unwrap();
        db.transition_load_status(load.id, LoadStatus::Dispatched, None, None, None).await.unwrap();
        let fetched = db.get_load_by_id(load.id).await.unwrap();
        assert_eq!(fetched.status, LoadStatus::Dispatched);
    }

    #[tokio::test]
    async fn test_next_load_number_sequences() {
        let (db, _dir) = test_db().await;
        let n1 = db.next_load_number(2026).await.unwrap();
        assert_eq!(n1, "LD-2026-0001");
        let mut load = sample_load();
        load.load_number = n1.clone();
        db.insert_load(&load).await.unwrap();
        let n2 = db.next_load_number(2026).await.unwrap();
        assert_eq!(n2, "LD-2026-0002");
    }

    #[tokio::test]
    async fn test_any_load_references_facility() {
        let (db, _dir) = test_db().await;
        let fac_id = uuid::Uuid::new_v4();
        let mut load = sample_load();
        load.stops = vec![crate::models::Stop {
            sequence: 1,
            stop_type: crate::models::StopType::Pickup,
            service_type: crate::models::ServiceType::LiveLoad,
            facility_id: fac_id,
            scheduled_arrive: "2026-05-10T08:00:00".into(),
            scheduled_arrive_end: None, actual_arrive: None, actual_depart: None,
            expected_dwell_minutes: None, detention_free_minutes: None,
            detention_grace_minutes: None,
            notes: None, blob_ids: vec![],
            timezone: Some("America/Chicago".into()),
            actual_arrive_utc: None, actual_depart_utc: None,
        }];
        db.insert_load(&load).await.unwrap();
        assert!(db.any_load_references_facility(fac_id).await.unwrap());
        assert!(!db.any_load_references_facility(uuid::Uuid::new_v4()).await.unwrap());
    }

    #[tokio::test]
    async fn test_any_load_references_blob() {
        let (db, _dir) = test_db().await;
        let blob_id = uuid::Uuid::new_v4();
        let mut load = sample_load();
        load.blob_ids = vec![blob_id];
        db.insert_load(&load).await.unwrap();
        assert!(db.any_load_references_blob(blob_id).await.unwrap());
        assert!(!db.any_load_references_blob(uuid::Uuid::new_v4()).await.unwrap());
    }

    #[tokio::test]
    async fn test_loads_referencing_blob() {
        let (db, _dir) = test_db().await;
        let blob_id = uuid::Uuid::new_v4();
        let mut load = sample_load();
        load.blob_ids = vec![blob_id];
        db.insert_load(&load).await.unwrap();
        let refs = db.loads_referencing_blob(blob_id).await.unwrap();
        assert_eq!(refs, vec![load.id]);
        assert!(db.loads_referencing_blob(uuid::Uuid::new_v4()).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_update_load_miles() {
        let (db, _dir) = test_db().await;
        let load = sample_load();
        db.insert_load(&load).await.unwrap();
        db.update_load_miles(load.id, 385.5).await.unwrap();
        let fetched = db.get_load_by_id(load.id).await.unwrap();
        assert!((fetched.miles.unwrap() - 385.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_build_load_filter_valid_dates() {
        let filter = build_load_filter(None, None, &[], Some("2026-01-01T00:00:00Z"), Some("2026-12-31T23:59:59Z")).unwrap();
        let f = filter.unwrap();
        assert!(f.contains("created_at >= '2026-01-01T00:00:00Z'"));
        assert!(f.contains("created_at <= '2026-12-31T23:59:59Z'"));
    }

    #[test]
    fn test_build_load_filter_invalid_from_returns_bad_request() {
        let result = build_load_filter(None, None, &[], Some("' OR 1=1--"), None);
        assert!(matches!(result, Err(AppError::BadRequest(_))));
    }

    #[test]
    fn test_build_load_filter_invalid_to_returns_bad_request() {
        let result = build_load_filter(None, None, &[], None, Some("not-a-date"));
        assert!(matches!(result, Err(AppError::BadRequest(_))));
    }
}
