// src/db/trip_ops.rs
use crate::{
    db::{trip_schema, DbClient},
    error::AppError,
    models::trip::{TripListItem, TripRecord, TripStatus, TripStop},
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
    pub async fn insert_trip(&self, record: &TripRecord) -> Result<(), AppError> {
        let batch = trip_to_batch(record, self.embed_dim)?;
        let schema = trip_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.trip_table.add(reader).execute().await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))?;

        // Cascade: if both load_id and driver_id are present and load is planned, transition to assigned
        if let (Some(load_id), Some(_driver_id)) = (record.load_id, record.driver_id) {
            if let Ok(load) = self.get_load_by_id(load_id).await {
                if load.status == crate::models::LoadStatus::Planned {
                    let _ = self.transition_load_status(
                        load_id, crate::models::LoadStatus::Assigned, None, None, None,
                    ).await;
                }
            }
        }

        Ok(())
    }

    pub async fn get_trip(&self, id: Uuid) -> Result<TripRecord, AppError> {
        let stream = self.trip_table.query()
            .only_if(format!("id = '{id}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        batches_to_trips(collect_stream(stream).await?)?
            .into_iter().next()
            .ok_or(AppError::NotFound)
    }

    pub async fn list_trips(
        &self,
        load_id: Option<Uuid>,
        driver_id: Option<Uuid>,
        status: Option<&str>,
    ) -> Result<Vec<TripListItem>, AppError> {
        let filter = build_trip_filter(load_id, driver_id, status);
        let mut q = self.trip_table.query();
        if let Some(f) = filter { q = q.only_if(f); }
        let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let items: Vec<TripListItem> = batches_to_trips(collect_stream(stream).await?)?
            .into_iter().map(TripListItem::from).collect();
        Ok(items)
    }

    async fn upsert_trip(&self, record: &TripRecord) -> Result<(), AppError> {
        let batch = trip_to_batch(record, self.embed_dim)?;
        let schema = trip_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.trip_table.merge_insert(&["id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn update_trip_resources(
        &self,
        id: Uuid,
        driver_id: Option<Uuid>,
        truck_id: Option<Uuid>,
        trailer_ids: Vec<Uuid>,
    ) -> Result<(), AppError> {
        let mut record = self.get_trip(id).await?;
        record.driver_id = driver_id;
        record.truck_id = truck_id;
        record.trailer_ids = trailer_ids;
        record.updated_at = chrono::Utc::now();
        self.upsert_trip(&record).await
    }

    pub async fn update_trip_mileage(
        &self, id: Uuid,
        deadhead_miles: Option<f64>,
        loaded_miles: Option<f64>,
        total_miles: Option<f64>,
        segment_miles: Vec<f64>,
    ) -> Result<TripRecord, AppError> {
        let mut record = self.get_trip(id).await?;
        record.deadhead_miles = deadhead_miles;
        record.loaded_miles = loaded_miles;
        record.total_miles = total_miles;
        record.segment_miles = segment_miles;
        record.updated_at = Utc::now();
        self.upsert_trip(&record).await?;
        Ok(record)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_trip_metadata(
        &self, id: Uuid,
        load_id: Option<Uuid>,
        sequence: Option<u32>,
        stops: Option<Vec<TripStop>>,
        notes: Option<String>,
        embedding: Option<Vec<f32>>,
        blob_ids: Option<Vec<Uuid>>,
    ) -> Result<TripRecord, AppError> {
        let mut record = self.get_trip(id).await?;
        if let Some(v) = load_id { record.load_id = Some(v); }
        if let Some(v) = sequence { record.sequence = v; }
        if let Some(v) = stops { record.stops = v; }
        if let Some(v) = notes { record.notes = Some(v); }
        if let Some(v) = embedding { record.embedding = Some(v); }
        if let Some(v) = blob_ids { record.blob_ids = v; }
        record.updated_at = Utc::now();
        self.upsert_trip(&record).await?;
        Ok(record)
    }

    /// Sets `previous_trip_id` to the given value (Some = set, omitted handled by caller).
    /// Caller is responsible for re-computing mileage after a change.
    pub async fn update_trip_previous_trip_id(
        &self, id: Uuid, previous_trip_id: Option<Uuid>,
    ) -> Result<TripRecord, AppError> {
        let mut record = self.get_trip(id).await?;
        record.previous_trip_id = previous_trip_id;
        record.updated_at = Utc::now();
        self.upsert_trip(&record).await?;
        Ok(record)
    }

    pub async fn transition_trip_status(
        &self, id: Uuid, new_status: TripStatus,
    ) -> Result<TripRecord, AppError> {
        let mut record = self.get_trip(id).await?;
        if !record.status.can_transition_to(&new_status) {
            return Err(AppError::Conflict(format!(
                "cannot transition from '{}' to '{}'",
                record.status.as_str(), new_status.as_str()
            )));
        }
        record.status = new_status;
        record.updated_at = Utc::now();
        self.upsert_trip(&record).await?;
        Ok(record)
    }

    pub async fn update_trip_stop(
        &self, id: Uuid, seq: u32,
        actual_arrive: Option<String>,
        actual_depart: Option<String>,
    ) -> Result<TripRecord, AppError> {
        let mut record = self.get_trip(id).await?;
        let stop = record.stops.iter_mut()
            .find(|s| s.sequence == seq)
            .ok_or(AppError::NotFound)?;
        if let Some(v) = actual_arrive { stop.actual_arrive = Some(v); }
        if let Some(v) = actual_depart { stop.actual_depart = Some(v); }
        record.updated_at = Utc::now();
        self.upsert_trip(&record).await?;
        Ok(record)
    }

    pub async fn delete_trip(&self, id: Uuid) -> Result<(), AppError> {
        let record = self.get_trip(id).await?;
        match record.status {
            TripStatus::InTransit | TripStatus::Delivered | TripStatus::Completed => {
                return Err(AppError::Conflict(format!(
                    "cannot cancel trip with status '{}'", record.status.as_str()
                )));
            }
            TripStatus::Cancelled => return self.hard_delete_trip(id).await,
            _ => {}
        }
        self.transition_trip_status(id, TripStatus::Cancelled).await?;
        Ok(())
    }

    pub async fn hard_delete_trip(&self, id: Uuid) -> Result<(), AppError> {
        self.trip_table.delete(&format!("id = '{id}'")).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn next_trip_number(&self, year: i32) -> Result<String, AppError> {
        let prefix = format!("T-{year}-");
        let stream = self.trip_table.query()
            .only_if(format!("trip_number LIKE '{prefix}%'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let records = batches_to_trips(collect_stream(stream).await?)?;
        let max_n = records.iter()
            .filter_map(|r| r.trip_number.strip_prefix(&prefix))
            .filter_map(|s| s.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        Ok(format!("{prefix}{:04}", max_n + 1))
    }

    pub async fn list_trips_for_load(&self, load_id: Uuid) -> Result<Vec<TripRecord>, AppError> {
        let id_str = load_id.to_string();
        let stream = self.trip_table.query()
            .only_if(format!("load_id = '{id_str}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        batches_to_trips(collect_stream(stream).await?)
    }

    pub async fn count_active_trips_for_load(&self, load_id: Uuid) -> Result<usize, AppError> {
        let id_str = load_id.to_string();
        let filter = format!(
            "load_id = '{id_str}' AND status NOT IN ('cancelled', 'delivered', 'completed')"
        );
        self.trip_table.count_rows(Some(filter)).await
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_last_trip_for_driver(&self, driver_id: Uuid) -> Result<Option<TripRecord>, AppError> {
        let id_str = driver_id.to_string();
        let stream = self.trip_table.query()
            .only_if(format!("driver_id = '{id_str}' AND status != 'cancelled'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let mut trips = batches_to_trips(collect_stream(stream).await?)?;
        trips.sort_by_key(|t| std::cmp::Reverse(t.created_at));
        Ok(trips.into_iter().next())
    }

    pub async fn any_trip_references_blob(&self, blob_id: Uuid) -> Result<bool, AppError> {
        let id_str = blob_id.to_string();
        // Use JSON string boundaries to avoid false positives from UUID substrings
        let count = self.trip_table
            .count_rows(Some(format!("blob_ids LIKE '%\"{id_str}\"%'")))
            .await.map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(count > 0)
    }

    /// Ids of trips that reference `blob_id` in their `blob_ids`.
    /// Used for the MCP `attached_to` reverse lookup.
    pub async fn trips_referencing_blob(&self, blob_id: Uuid) -> Result<Vec<Uuid>, AppError> {
        let id_str = blob_id.to_string();
        let stream = self.trip_table.query()
            .only_if(format!("blob_ids LIKE '%\"{id_str}\"%'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(batches_to_trips(collect_stream(stream).await?)?
            .into_iter().map(|r| r.id).collect())
    }
}

// --- Helpers ---

fn trip_to_batch(record: &TripRecord, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let schema = trip_schema(embed_dim);

    let trailer_ids_json = serde_json::to_string(&record.trailer_ids)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let stops_json = serde_json::to_string(&record.stops)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let id_str = record.id.to_string();
    let load_id_str = record.load_id.map(|u| u.to_string());
    let driver_id_str = record.driver_id.map(|u| u.to_string());
    let truck_id_str = record.truck_id.map(|u| u.to_string());
    let created_at_str = record.created_at.to_rfc3339();
    let updated_at_str = record.updated_at.to_rfc3339();
    let load_number_str = record.load_number.as_deref();
    let previous_trip_id_str = record.previous_trip_id.map(|u| u.to_string());
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
        Arc::new(StringArray::from(vec![record.trip_number.as_str()])),
        Arc::new(StringArray::from(vec![load_id_str.as_deref()])),
        Arc::new(Int64Array::from(vec![record.sequence as i64])),
        Arc::new(StringArray::from(vec![driver_id_str.as_deref()])),
        Arc::new(StringArray::from(vec![truck_id_str.as_deref()])),
        Arc::new(StringArray::from(vec![trailer_ids_json.as_str()])),
        Arc::new(StringArray::from(vec![record.status.as_str()])),
        Arc::new(StringArray::from(vec![stops_json.as_str()])),
        Arc::new(StringArray::from(vec![record.notes.as_deref()])),
        embedding_col,
        Arc::new(Int64Array::from(vec![record.owner_id])),
        Arc::new(StringArray::from(vec![created_at_str.as_str()])),
        Arc::new(StringArray::from(vec![updated_at_str.as_str()])),
        Arc::new(StringArray::from(vec![load_number_str])),
        Arc::new(StringArray::from(vec![previous_trip_id_str.as_deref()])),
        Arc::new(Float64Array::from(vec![record.deadhead_miles])),
        Arc::new(Float64Array::from(vec![record.loaded_miles])),
        Arc::new(Float64Array::from(vec![record.total_miles])),
        Arc::new(StringArray::from(vec![
            if record.segment_miles.is_empty() { None }
            else { Some(serde_json::to_string(&record.segment_miles).unwrap_or_default()) }
        ])),
        Arc::new(StringArray::from(vec![blob_ids_json.as_str()])),
        Arc::new(Float64Array::from(vec![record.loaded_rate_per_mile])),
        Arc::new(Float64Array::from(vec![record.deadhead_rate_per_mile])),
        Arc::new(Float64Array::from(vec![record.extra_stop_fee])),
        Arc::new(Float64Array::from(vec![record.detention_rate_per_hour])),
        Arc::new(Int64Array::from(vec![record.free_dwell_minutes.map(|v| v as i64)])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_trips(batches: Vec<RecordBatch>) -> Result<Vec<TripRecord>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() { out.push(row_to_trip(batch, i)?); }
    }
    Ok(out)
}

fn row_to_trip(batch: &RecordBatch, i: usize) -> Result<TripRecord, AppError> {
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
    let opt_i64 = |name: &str| -> Option<i64> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i)) })
    };

    let stops: Vec<TripStop> = serde_json::from_str(&str_col("stops")).unwrap_or_default();
    let trailer_ids: Vec<Uuid> = serde_json::from_str(&str_col("trailer_ids")).unwrap_or_default();

    let load_id = opt_str("load_id")
        .map(|s| s.parse::<Uuid>().map_err(|e| AppError::Internal(e.to_string())))
        .transpose()?;
    let driver_id = opt_str("driver_id")
        .map(|s| s.parse::<Uuid>().map_err(|e| AppError::Internal(e.to_string())))
        .transpose()?;
    let truck_id = opt_str("truck_id")
        .map(|s| s.parse::<Uuid>().map_err(|e| AppError::Internal(e.to_string())))
        .transpose()?;

    let embedding = batch.column_by_name("embedding")
        .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
        .and_then(|fsl| {
            if fsl.is_null(i) { return None; }
            let values = fsl.value(i);
            values.as_any().downcast_ref::<Float32Array>()
                .map(|fa| (0..fa.len()).map(|j| fa.value(j)).collect::<Vec<f32>>())
        });

    Ok(TripRecord {
        id: str_col("id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        trip_number: str_col("trip_number"),
        load_id,
        load_number: opt_str("load_number"),
        previous_trip_id: opt_str("previous_trip_id")
            .map(|s| s.parse::<Uuid>().map_err(|e| AppError::Internal(e.to_string())))
            .transpose()?,
        deadhead_miles: opt_f64("deadhead_miles"),
        loaded_miles: opt_f64("loaded_miles"),
        total_miles: opt_f64("total_miles"),
        segment_miles: opt_str("segment_miles")
            .and_then(|s| serde_json::from_str::<Vec<f64>>(&s).ok())
            .unwrap_or_default(),
        sequence: i64_col("sequence") as u32,
        driver_id,
        truck_id,
        trailer_ids,
        status: str_col("status").parse().map_err(|e: String| AppError::Internal(e))?,
        stops,
        notes: opt_str("notes"),
        blob_ids: serde_json::from_str(&str_col("blob_ids")).unwrap_or_default(),
        loaded_rate_per_mile: opt_f64("loaded_rate_per_mile"),
        deadhead_rate_per_mile: opt_f64("deadhead_rate_per_mile"),
        extra_stop_fee: opt_f64("extra_stop_fee"),
        detention_rate_per_hour: opt_f64("detention_rate_per_hour"),
        free_dwell_minutes: opt_i64("free_dwell_minutes").map(|v| v as u32),
        embedding,
        owner_id: i64_col("owner_id"),
        created_at: str_col("created_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
        updated_at: str_col("updated_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
    })
}

fn build_trip_filter(
    load_id: Option<Uuid>,
    driver_id: Option<Uuid>,
    status: Option<&str>,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(id) = load_id { parts.push(format!("load_id = '{id}'")); }
    if let Some(id) = driver_id { parts.push(format!("driver_id = '{id}'")); }
    if let Some(s) = status { parts.push(format!("status = '{}'", s.replace('\'', "''"))); }
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
    use crate::models::trip::{TripStatus, TripStopType};
    use tempfile::TempDir;

    async fn test_db() -> (DbClient, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        (db, dir)
    }

    fn sample_trip() -> TripRecord {
        let now = chrono::Utc::now();
        TripRecord {
            id: uuid::Uuid::new_v4(),
            trip_number: "T-2026-0001".into(),
            load_id: None,
            load_number: None,
            previous_trip_id: None,
            deadhead_miles: None,
            loaded_miles: None,
            total_miles: None,
            segment_miles: vec![],
            sequence: 0,
            driver_id: None,
            truck_id: None,
            trailer_ids: vec![],
            status: TripStatus::Planned,
            stops: vec![
                TripStop {
                    sequence: 0,
                    stop_type: TripStopType::Pickup,
                    facility_id: None,
                    name: Some("Chicago Hub".into()),
                    address: None,
                    load_stop_index: None,
                    scheduled_arrive: None,
                    scheduled_arrive_end: None,
                    actual_arrive: None,
                    actual_depart: None,
                    expected_dwell_minutes: None,
                    detention_free_minutes: None,
                    detention_grace_minutes: None,
                    notes: None,
                    timezone: None,
                    actual_arrive_utc: None,
                    actual_depart_utc: None,
                },
            ],
            notes: Some("test trip".into()),
            blob_ids: vec![],
            loaded_rate_per_mile: None,
            deadhead_rate_per_mile: None,
            extra_stop_fee: None,
            detention_rate_per_hour: None,
            free_dwell_minutes: None,
            embedding: None,
            owner_id: 0,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_insert_and_get_trip() {
        let (db, _dir) = test_db().await;
        let trip = sample_trip();
        db.insert_trip(&trip).await.unwrap();
        let fetched = db.get_trip(trip.id).await.unwrap();
        assert_eq!(fetched.id, trip.id);
        assert_eq!(fetched.trip_number, "T-2026-0001");
        assert_eq!(fetched.stops.len(), 1);
        assert_eq!(fetched.stops[0].name.as_deref(), Some("Chicago Hub"));
    }

    #[tokio::test]
    async fn test_get_not_found() {
        let (db, _dir) = test_db().await;
        assert!(matches!(db.get_trip(uuid::Uuid::new_v4()).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_soft_delete() {
        let (db, _dir) = test_db().await;
        let trip = sample_trip();
        db.insert_trip(&trip).await.unwrap();
        db.delete_trip(trip.id).await.unwrap();
        let fetched = db.get_trip(trip.id).await.unwrap();
        assert_eq!(fetched.status, TripStatus::Cancelled);
    }

    #[tokio::test]
    async fn test_next_trip_number_sequences() {
        let (db, _dir) = test_db().await;
        let n1 = db.next_trip_number(2026).await.unwrap();
        assert_eq!(n1, "T-2026-0001");
        let mut trip = sample_trip();
        trip.trip_number = n1.clone();
        db.insert_trip(&trip).await.unwrap();
        let n2 = db.next_trip_number(2026).await.unwrap();
        assert_eq!(n2, "T-2026-0002");
    }

    #[tokio::test]
    async fn test_soft_delete_in_transit_returns_conflict() {
        let (db, _dir) = test_db().await;
        let mut trip = sample_trip();
        trip.status = TripStatus::InTransit;
        db.insert_trip(&trip).await.unwrap();
        assert!(matches!(db.delete_trip(trip.id).await, Err(AppError::Conflict(_))));
    }

    #[tokio::test]
    async fn test_get_trip_not_found_returns_not_found() {
        let (db, _dir) = test_db().await;
        assert!(matches!(db.get_trip(uuid::Uuid::new_v4()).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_get_last_trip_for_driver_returns_most_recent_non_cancelled() {
        let (db, _dir) = test_db().await;
        let driver_id = uuid::Uuid::new_v4();

        // First trip
        let mut t1 = sample_trip();
        t1.driver_id = Some(driver_id);
        t1.trip_number = "T-2026-0001".into();
        t1.created_at = chrono::Utc::now() - chrono::Duration::minutes(10);
        db.insert_trip(&t1).await.unwrap();

        // Second trip — newer
        let mut t2 = sample_trip();
        t2.driver_id = Some(driver_id);
        t2.trip_number = "T-2026-0002".into();
        t2.created_at = chrono::Utc::now();
        db.insert_trip(&t2).await.unwrap();

        let last = db.get_last_trip_for_driver(driver_id).await.unwrap();
        assert!(last.is_some());
        assert_eq!(last.unwrap().trip_number, "T-2026-0002");
    }

    #[tokio::test]
    async fn test_get_last_trip_for_driver_excludes_cancelled() {
        let (db, _dir) = test_db().await;
        let driver_id = uuid::Uuid::new_v4();

        let mut t1 = sample_trip();
        t1.driver_id = Some(driver_id);
        t1.status = TripStatus::Cancelled;
        t1.created_at = chrono::Utc::now();
        db.insert_trip(&t1).await.unwrap();

        let last = db.get_last_trip_for_driver(driver_id).await.unwrap();
        assert!(last.is_none(), "cancelled trips should be excluded");
    }

    #[tokio::test]
    async fn test_get_last_trip_for_driver_no_trips_returns_none() {
        let (db, _dir) = test_db().await;
        let result = db.get_last_trip_for_driver(uuid::Uuid::new_v4()).await.unwrap();
        assert!(result.is_none());
    }
}
