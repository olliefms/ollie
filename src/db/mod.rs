// src/db/mod.rs
pub mod blob_ops;
pub mod dispatcher_api_key_ops;
pub mod dispatcher_ops;
pub mod driver_credentials_ops;
pub mod driver_ops;
pub mod event_ops;
pub mod facility_ops;
pub mod load_ops;
pub mod trailer_ops;
pub mod trip_ops;
pub mod truck_ops;

use crate::error::AppError;
use arrow_array::{
    FixedSizeListArray, Float64Array, Int32Array, Int64Array, RecordBatch,
    RecordBatchIterator, RecordBatchReader, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use lancedb::table::NewColumnTransform;
use lancedb::Table;
use std::sync::Arc;

pub struct DbClient {
    pub blob_table: Table,
    pub dispatcher_table: Table,
    pub dispatcher_credentials_table: Table,
    pub dispatcher_api_key_table: Table,
    pub driver_credentials_table: Table,
    pub driver_passkey_credentials_table: Table,
    pub driver_table: Table,
    pub event_table: Table,
    pub facility_table: Table,
    pub load_table: Table,
    pub trailer_table: Table,
    pub trip_table: Table,
    pub truck_table: Table,
    pub embed_dim: usize,
}

impl DbClient {
    pub async fn new(path: &str, embed_dim: usize) -> Result<Self, AppError> {
        let conn = lancedb::connect(path)
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;

        let blob_table = open_or_create_blob(&conn, embed_dim).await?;

        let facility_table = open_or_create_facility(&conn, embed_dim).await?;

        let load_table = open_or_create(&conn, "loads", load_schema(embed_dim), |schema| {
            empty_load_batch(schema, embed_dim)
        }).await?;

        let driver_table = open_or_create(&conn, "drivers", driver_schema(embed_dim), |schema| {
            empty_driver_batch(schema, embed_dim)
        }).await?;

        let truck_table = open_or_create(&conn, "trucks", truck_schema(embed_dim), |schema| {
            empty_truck_batch(schema, embed_dim)
        }).await?;

        let trailer_table = open_or_create(&conn, "trailers", trailer_schema(embed_dim), |schema| {
            empty_trailer_batch(schema, embed_dim)
        }).await?;

        let trip_table = open_or_create_trip(&conn, embed_dim).await?;

        let event_table = open_or_create(&conn, "events", event_schema(embed_dim), |schema| {
            empty_event_batch(schema, embed_dim)
        }).await?;

        let driver_credentials_table = open_or_create(
            &conn,
            "driver_credentials",
            driver_credentials_schema(),
            empty_driver_credentials_batch,
        ).await?;

        let driver_passkey_credentials_table = open_or_create(
            &conn,
            "driver_passkey_credentials",
            driver_passkey_credentials_schema(),
            empty_driver_passkey_credentials_batch,
        ).await?;

        let dispatcher_table = open_or_create(
            &conn,
            "dispatchers",
            dispatcher_schema(),
            empty_dispatcher_batch,
        ).await?;

        let dispatcher_credentials_table = open_or_create(
            &conn,
            "dispatcher_credentials",
            dispatcher_credentials_schema(),
            empty_dispatcher_credentials_batch,
        ).await?;

        let dispatcher_api_key_table = open_or_create(
            &conn,
            "dispatcher_api_keys",
            dispatcher_api_key_schema(),
            empty_dispatcher_api_key_batch,
        ).await?;

        Ok(Self {
            blob_table,
            dispatcher_table,
            dispatcher_credentials_table,
            dispatcher_api_key_table,
            driver_credentials_table,
            driver_passkey_credentials_table,
            driver_table,
            event_table,
            facility_table,
            load_table,
            trailer_table,
            trip_table,
            truck_table,
            embed_dim,
        })
    }
}

async fn open_or_create<F>(
    conn: &lancedb::Connection,
    name: &str,
    schema: Arc<Schema>,
    make_batch: F,
) -> Result<Table, AppError>
where
    F: FnOnce(Arc<Schema>) -> Result<RecordBatch, AppError>,
{
    match conn.open_table(name).execute().await {
        Ok(t) => Ok(t),
        Err(_) => {
            let batch = make_batch(schema.clone())?;
            let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
            let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
            conn.create_table(name, reader).execute().await
                .map_err(|e| AppError::Internal(e.to_string()))
        }
    }
}

async fn open_or_create_trip(conn: &lancedb::Connection, embed_dim: usize) -> Result<Table, AppError> {
    let schema = trip_schema(embed_dim);
    match conn.open_table("trips").execute().await {
        Err(_) => {
            let batch = empty_trip_batch(schema.clone(), embed_dim)?;
            let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
            let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
            conn.create_table("trips", reader).execute().await
                .map_err(|e| AppError::Internal(e.to_string()))
        }
        Ok(table) => {
            let existing = table.schema().await.map_err(|e| AppError::Internal(e.to_string()))?;
            let mut transforms: Vec<(String, String)> = Vec::new();
            if existing.field_with_name("load_number").is_err() {
                transforms.push(("load_number".into(), "CAST(NULL AS string)".into()));
            }
            if existing.field_with_name("previous_trip_id").is_err() {
                transforms.push(("previous_trip_id".into(), "CAST(NULL AS string)".into()));
            }
            if existing.field_with_name("deadhead_miles").is_err() {
                transforms.push(("deadhead_miles".into(), "CAST(NULL AS double)".into()));
            }
            if existing.field_with_name("loaded_miles").is_err() {
                transforms.push(("loaded_miles".into(), "CAST(NULL AS double)".into()));
            }
            if existing.field_with_name("total_miles").is_err() {
                transforms.push(("total_miles".into(), "CAST(NULL AS double)".into()));
            }
            if existing.field_with_name("segment_miles").is_err() {
                transforms.push(("segment_miles".into(), "CAST(NULL AS string)".into()));
            }
            if !transforms.is_empty() {
                tracing::info!("migrating trips table: adding {} column(s)", transforms.len());
                table.add_columns(NewColumnTransform::SqlExpressions(transforms), None).await
                    .map_err(|e| AppError::Internal(format!("trip schema migration failed: {e}")))?;
            }
            Ok(table)
        }
    }
}

async fn open_or_create_blob(conn: &lancedb::Connection, embed_dim: usize) -> Result<Table, AppError> {
    let schema = blob_schema(embed_dim);
    match conn.open_table("blobs").execute().await {
        Err(_) => {
            let batch = empty_blob_batch(schema.clone(), embed_dim)?;
            let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
            let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
            conn.create_table("blobs", reader).execute().await
                .map_err(|e| AppError::Internal(e.to_string()))
        }
        Ok(table) => {
            let existing = table.schema().await.map_err(|e| AppError::Internal(e.to_string()))?;
            let mut transforms: Vec<(String, String)> = Vec::new();
            if existing.field_with_name("visibility").is_err() {
                transforms.push(("visibility".into(), "'private'".into()));
            }
            if existing.field_with_name("uploaded_by").is_err() {
                transforms.push(("uploaded_by".into(), "CAST(NULL AS string)".into()));
            }
            if !transforms.is_empty() {
                tracing::info!("migrating blobs table: adding {} column(s)", transforms.len());
                table.add_columns(NewColumnTransform::SqlExpressions(transforms), None).await
                    .map_err(|e| AppError::Internal(format!("blob schema migration failed: {e}")))?;
            }
            Ok(table)
        }
    }
}

async fn open_or_create_facility(conn: &lancedb::Connection, embed_dim: usize) -> Result<Table, AppError> {
    let schema = facility_schema(embed_dim);
    match conn.open_table("facilities").execute().await {
        Err(_) => {
            let batch = empty_facility_batch(schema.clone(), embed_dim)?;
            let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
            let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
            conn.create_table("facilities", reader).execute().await
                .map_err(|e| AppError::Internal(e.to_string()))
        }
        Ok(table) => {
            let existing_schema = table.schema().await.map_err(|e| AppError::Internal(e.to_string()))?;
            let needs_geocode_failure_count = existing_schema.field_with_name("geocode_failure_count").is_err();
            let needs_geocode_status = existing_schema.field_with_name("geocode_status").is_err();
            let mut transforms: Vec<(String, String)> = Vec::new();
            if needs_geocode_failure_count {
                transforms.push(("geocode_failure_count".into(), "CAST(0 AS BIGINT)".into()));
            }
            if needs_geocode_status {
                transforms.push(("geocode_status".into(), "'pending'".into()));
            }
            if !transforms.is_empty() {
                tracing::info!("migrating facilities table: adding {} column(s)", transforms.len());
                table.add_columns(NewColumnTransform::SqlExpressions(transforms), None).await
                    .map_err(|e| AppError::Internal(format!("facility schema migration failed: {e}")))?;
            }
            Ok(table)
        }
    }
}

pub fn blob_schema(embed_dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("owner_id", DataType::Int64, false),
        Field::new("checksum", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("mime_type", DataType::Utf8, false),
        Field::new("size", DataType::Int64, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("error", DataType::Utf8, true),
        Field::new("summary", DataType::Utf8, true),
        Field::new("tags", DataType::Utf8, false),
        Field::new("embedding", DataType::FixedSizeList(
            Arc::new(Field::new("item", DataType::Float32, true)),
            embed_dim as i32,
        ), true),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
        Field::new("visibility", DataType::Utf8, false),
        Field::new("uploaded_by", DataType::Utf8, true),
    ]))
}

pub fn facility_schema(embed_dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("owner_id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("address", DataType::Utf8, false),
        Field::new("normalized_address", DataType::Utf8, true),
        Field::new("lat", DataType::Float64, true),
        Field::new("lng", DataType::Float64, true),
        Field::new("geocode_status", DataType::Utf8, false),
        Field::new("contacts", DataType::Utf8, false),
        Field::new("notes", DataType::Utf8, true),
        Field::new("tags", DataType::Utf8, false),
        Field::new("blob_ids", DataType::Utf8, false),
        Field::new("avg_dwell_minutes", DataType::Float64, true),
        Field::new("dwell_sample_count", DataType::Int64, false),
        Field::new("geocode_failure_count", DataType::Int64, false),
        Field::new("embedding", DataType::FixedSizeList(
            Arc::new(Field::new("item", DataType::Float32, true)),
            embed_dim as i32,
        ), true),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
    ]))
}

pub fn load_schema(embed_dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("load_number", DataType::Utf8, false),
        Field::new("owner_id", DataType::Int64, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("customer_name", DataType::Utf8, false),
        Field::new("customer_ref", DataType::Utf8, true),
        Field::new("stops", DataType::Utf8, false),
        Field::new("rate_items", DataType::Utf8, false),
        Field::new("commodity", DataType::Utf8, true),
        Field::new("weight_lbs", DataType::Float64, true),
        Field::new("miles", DataType::Float64, true),
        Field::new("notes", DataType::Utf8, true),
        Field::new("tags", DataType::Utf8, false),
        Field::new("blob_ids", DataType::Utf8, false),
        Field::new("invoice_number", DataType::Utf8, true),
        Field::new("invoice_date", DataType::Utf8, true),
        Field::new("cancellation_reason", DataType::Utf8, true),
        Field::new("embedding", DataType::FixedSizeList(
            Arc::new(Field::new("item", DataType::Float32, true)),
            embed_dim as i32,
        ), true),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
    ]))
}

pub fn driver_schema(embed_dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("phone", DataType::Utf8, true),
        Field::new("email", DataType::Utf8, true),
        Field::new("license_number", DataType::Utf8, true),
        Field::new("license_state", DataType::Utf8, true),
        Field::new("license_expiry", DataType::Utf8, true),
        Field::new("status", DataType::Utf8, false),
        Field::new("notes", DataType::Utf8, true),
        Field::new("embedding", DataType::FixedSizeList(
            Arc::new(Field::new("item", DataType::Float32, true)),
            embed_dim as i32,
        ), true),
        Field::new("owner_id", DataType::Int64, false),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
    ]))
}

fn empty_driver_batch(schema: Arc<Schema>, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let nulls: Vec<Option<Vec<Option<f32>>>> = vec![];
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(FixedSizeListArray::from_iter_primitive::<
            arrow_array::types::Float32Type, _, _
        >(nulls, embed_dim as i32)),
        Arc::new(Int64Array::from(Vec::<i64>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn empty_blob_batch(schema: Arc<Schema>, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let nulls: Vec<Option<Vec<Option<f32>>>> = vec![];
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(Int64Array::from(Vec::<i64>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(Int64Array::from(Vec::<i64>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(FixedSizeListArray::from_iter_primitive::<
            arrow_array::types::Float32Type, _, _
        >(nulls, embed_dim as i32)),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn empty_facility_batch(schema: Arc<Schema>, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let nulls: Vec<Option<Vec<Option<f32>>>> = vec![];
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(Int64Array::from(Vec::<i64>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),
        Arc::new(Int64Array::from(Vec::<i64>::new())),
        Arc::new(Int64Array::from(Vec::<i64>::new())),
        Arc::new(FixedSizeListArray::from_iter_primitive::<
            arrow_array::types::Float32Type, _, _
        >(nulls, embed_dim as i32)),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

pub fn event_schema(embed_dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("entity_type", DataType::Utf8, false),
        Field::new("entity_id", DataType::Utf8, false),
        Field::new("event_type", DataType::Utf8, false),
        Field::new("payload", DataType::Utf8, true),
        Field::new("actor", DataType::Utf8, true),
        Field::new("occurred_at", DataType::Utf8, false),
        Field::new("embedding", DataType::FixedSizeList(
            Arc::new(Field::new("item", DataType::Float32, true)),
            embed_dim as i32,
        ), true),
    ]))
}

fn empty_event_batch(schema: Arc<Schema>, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let nulls: Vec<Option<Vec<Option<f32>>>> = vec![];
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(FixedSizeListArray::from_iter_primitive::<
            arrow_array::types::Float32Type, _, _
        >(nulls, embed_dim as i32)),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

pub fn truck_schema(embed_dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("unit_number", DataType::Utf8, false),
        Field::new("year", DataType::Int64, true),
        Field::new("make", DataType::Utf8, true),
        Field::new("model", DataType::Utf8, true),
        Field::new("vin", DataType::Utf8, true),
        Field::new("plate", DataType::Utf8, true),
        Field::new("plate_state", DataType::Utf8, true),
        Field::new("status", DataType::Utf8, false),
        Field::new("notes", DataType::Utf8, true),
        Field::new("embedding", DataType::FixedSizeList(
            Arc::new(Field::new("item", DataType::Float32, true)),
            embed_dim as i32,
        ), true),
        Field::new("owner_id", DataType::Int64, false),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
    ]))
}

fn empty_truck_batch(schema: Arc<Schema>, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let nulls: Vec<Option<Vec<Option<f32>>>> = vec![];
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(Int64Array::from(Vec::<Option<i64>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(FixedSizeListArray::from_iter_primitive::<
            arrow_array::types::Float32Type, _, _
        >(nulls, embed_dim as i32)),
        Arc::new(Int64Array::from(Vec::<i64>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

pub fn trailer_schema(embed_dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("unit_number", DataType::Utf8, false),
        Field::new("owner", DataType::Utf8, false),
        Field::new("owner_name", DataType::Utf8, true),
        Field::new("year", DataType::Int64, true),
        Field::new("make", DataType::Utf8, true),
        Field::new("trailer_type", DataType::Utf8, true),
        Field::new("length_ft", DataType::Float64, true),
        Field::new("vin", DataType::Utf8, true),
        Field::new("plate", DataType::Utf8, true),
        Field::new("plate_state", DataType::Utf8, true),
        Field::new("status", DataType::Utf8, false),
        Field::new("notes", DataType::Utf8, true),
        Field::new("embedding", DataType::FixedSizeList(
            Arc::new(Field::new("item", DataType::Float32, true)),
            embed_dim as i32,
        ), true),
        Field::new("owner_id", DataType::Int64, false),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
    ]))
}

fn empty_trailer_batch(schema: Arc<Schema>, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let nulls: Vec<Option<Vec<Option<f32>>>> = vec![];
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(Int64Array::from(Vec::<Option<i64>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(FixedSizeListArray::from_iter_primitive::<
            arrow_array::types::Float32Type, _, _
        >(nulls, embed_dim as i32)),
        Arc::new(Int64Array::from(Vec::<i64>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

pub fn trip_schema(embed_dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("trip_number", DataType::Utf8, false),
        Field::new("load_id", DataType::Utf8, true),
        Field::new("sequence", DataType::Int64, false),
        Field::new("driver_id", DataType::Utf8, true),
        Field::new("truck_id", DataType::Utf8, true),
        Field::new("trailer_ids", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("stops", DataType::Utf8, false),
        Field::new("notes", DataType::Utf8, true),
        Field::new("embedding", DataType::FixedSizeList(
            Arc::new(Field::new("item", DataType::Float32, true)),
            embed_dim as i32,
        ), true),
        Field::new("owner_id", DataType::Int64, false),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
        Field::new("load_number", DataType::Utf8, true),
        Field::new("previous_trip_id", DataType::Utf8, true),
        Field::new("deadhead_miles", DataType::Float64, true),
        Field::new("loaded_miles", DataType::Float64, true),
        Field::new("total_miles", DataType::Float64, true),
        Field::new("segment_miles", DataType::Utf8, true),  // JSON-encoded Vec<f64>
    ]))
}

fn empty_trip_batch(schema: Arc<Schema>, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let nulls: Vec<Option<Vec<Option<f32>>>> = vec![];
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(Int64Array::from(Vec::<i64>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(FixedSizeListArray::from_iter_primitive::<
            arrow_array::types::Float32Type, _, _
        >(nulls, embed_dim as i32)),
        Arc::new(Int64Array::from(Vec::<i64>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // load_number
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // previous_trip_id
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),  // deadhead_miles
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),  // loaded_miles
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),  // total_miles
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // segment_miles
    ]).map_err(|e| AppError::Internal(e.to_string()))
}


pub fn driver_credentials_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("driver_id", DataType::Utf8, false),
        Field::new("pin_hash", DataType::Utf8, true),
        Field::new("token_version", DataType::Int64, false),
        Field::new("failed_pin_attempts", DataType::Int64, false),
        Field::new("locked_until", DataType::Utf8, true),
        Field::new("updated_at", DataType::Utf8, false),
    ]))
}

fn empty_driver_credentials_batch(schema: Arc<Schema>) -> Result<RecordBatch, AppError> {
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(Int64Array::from(Vec::<i64>::new())),
        Arc::new(Int64Array::from(Vec::<i64>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

pub fn driver_passkey_credentials_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("credential_id", DataType::Utf8, false),
        Field::new("driver_id", DataType::Utf8, false),
        Field::new("public_key", DataType::Utf8, false),
        Field::new("counter", DataType::Int64, false),
        Field::new("transports", DataType::Utf8, false),
        Field::new("created_at", DataType::Utf8, false),
    ]))
}

fn empty_driver_passkey_credentials_batch(schema: Arc<Schema>) -> Result<RecordBatch, AppError> {
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(Int64Array::from(Vec::<i64>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

pub fn dispatcher_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("email", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
    ]))
}

fn empty_dispatcher_batch(schema: Arc<Schema>) -> Result<RecordBatch, AppError> {
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

pub fn dispatcher_credentials_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("dispatcher_id", DataType::Utf8, false),
        Field::new("password_hash", DataType::Utf8, false),
        Field::new("token_version", DataType::Int64, false),
        Field::new("failed_attempts", DataType::Int32, false),
        Field::new("locked_until", DataType::Utf8, true),
        Field::new("updated_at", DataType::Utf8, false),
    ]))
}

fn empty_dispatcher_credentials_batch(schema: Arc<Schema>) -> Result<RecordBatch, AppError> {
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(Int64Array::from(Vec::<i64>::new())),
        Arc::new(Int32Array::from(Vec::<i32>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

pub fn dispatcher_api_key_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("dispatcher_id", DataType::Utf8, false),
        Field::new("label", DataType::Utf8, false),
        Field::new("key_hash", DataType::Utf8, false),
        Field::new("key_prefix", DataType::Utf8, false),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("expires_at", DataType::Utf8, false),
        Field::new("revoked_at", DataType::Utf8, true),
        Field::new("last_used_at", DataType::Utf8, true),
    ]))
}

fn empty_dispatcher_api_key_batch(schema: Arc<Schema>) -> Result<RecordBatch, AppError> {
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn empty_load_batch(schema: Arc<Schema>, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let nulls: Vec<Option<Vec<Option<f32>>>> = vec![];
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(Int64Array::from(Vec::<i64>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(FixedSizeListArray::from_iter_primitive::<
            arrow_array::types::Float32Type, _, _
        >(nulls, embed_dim as i32)),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_db_client_creates_tables() {
        let dir = TempDir::new().unwrap();
        let client = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        assert_eq!(client.blob_table.count_rows(None).await.unwrap(), 0);
        assert_eq!(client.facility_table.count_rows(None).await.unwrap(), 0);
        assert_eq!(client.load_table.count_rows(None).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_blob_schema_has_fixed_size_embedding() {
        let schema = blob_schema(768);
        let field = schema.field_with_name("embedding").unwrap();
        assert!(matches!(field.data_type(), DataType::FixedSizeList(_, 768)));
    }

    #[tokio::test]
    async fn test_facility_schema_has_float64_lat_lng() {
        let schema = facility_schema(4);
        assert!(matches!(schema.field_with_name("lat").unwrap().data_type(), DataType::Float64));
        assert!(matches!(schema.field_with_name("lng").unwrap().data_type(), DataType::Float64));
    }

    #[tokio::test]
    async fn test_db_client_has_credential_tables() {
        let dir = TempDir::new().unwrap();
        let client = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        assert_eq!(client.driver_credentials_table.count_rows(None).await.unwrap(), 0);
        assert_eq!(client.driver_passkey_credentials_table.count_rows(None).await.unwrap(), 0);
        assert_eq!(client.dispatcher_table.count_rows(None).await.unwrap(), 0);
        assert_eq!(client.dispatcher_credentials_table.count_rows(None).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_db_client_has_dispatcher_api_key_table() {
        let dir = TempDir::new().unwrap();
        let client = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        assert_eq!(client.dispatcher_api_key_table.count_rows(None).await.unwrap(), 0);
    }

    // Guards against the recurring failure documented in AGENTS.md: writing an
    // Arrow DataType spelling (Utf8, Float64, …) inside a `CAST(NULL AS …)`
    // expression. The DataFusion SQL parser bundled with LanceDB rejects those
    // at migration time and crash-loops the server. This bug has shipped to
    // production in v1.10.0, v1.13.0, and v1.16.0.
    //
    // Forbidden tokens are assembled via `concat!` so the test does not match
    // itself when scanning the source.
    #[test]
    fn cast_expressions_use_sql_types_not_arrow_types() {
        let source = include_str!("mod.rs");
        let cutoff = source.find("#[cfg(test)]").unwrap_or(source.len());
        let prod = &source[..cutoff];

        let forbidden: &[&str] = &[
            concat!("Ut", "f8"),
            concat!("ut", "f8"),
            concat!("Float", "64"),
            concat!("float", "64"),
            concat!("Int", "64"),
            concat!("Int", "32"),
        ];

        let mut violations = Vec::new();
        for ty in forbidden {
            let needle = format!(" AS {})", ty);
            let mut search_from = 0;
            while let Some(rel) = prod[search_from..].find(&needle) {
                let idx = search_from + rel;
                let line_start = prod[..idx].rfind('\n').map(|n| n + 1).unwrap_or(0);
                let line_end = prod[idx..].find('\n').map(|n| idx + n).unwrap_or(prod.len());
                let line_no = prod[..idx].bytes().filter(|&b| b == b'\n').count() + 1;
                violations.push(format!(
                    "  src/db/mod.rs:{}: `{}` — {}",
                    line_no,
                    ty,
                    prod[line_start..line_end].trim()
                ));
                search_from = idx + needle.len();
            }
        }

        assert!(
            violations.is_empty(),
            "Found Arrow type name(s) in CAST expressions. Use DataFusion SQL \
             keywords (`string`, `double`, `bigint`, …), not Arrow DataType \
             names. See the \"Recurring AI-agent failure\" lesson in AGENTS.md.\n{}",
            violations.join("\n")
        );
    }
}
