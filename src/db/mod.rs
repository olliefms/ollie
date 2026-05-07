// src/db/mod.rs
pub mod blob_ops;
pub mod driver_ops;
pub mod event_ops;
pub mod facility_ops;
pub mod load_ops;
pub mod trailer_ops;
pub mod trip_ops;
pub mod truck_ops;

use crate::error::AppError;
use arrow_array::{
    FixedSizeListArray, Float64Array, Int64Array, RecordBatch,
    RecordBatchIterator, RecordBatchReader, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use lancedb::Table;
use std::sync::Arc;

pub struct DbClient {
    pub blob_table: Table,
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

        let blob_table = open_or_create(&conn, "blobs", blob_schema(embed_dim), |schema| {
            empty_blob_batch(schema, embed_dim)
        }).await?;

        let facility_table = open_or_create(&conn, "facilities", facility_schema(embed_dim), |schema| {
            empty_facility_batch(schema, embed_dim)
        }).await?;

        let load_table = open_or_create(&conn, "loads", load_schema(embed_dim), |schema| {
            empty_load_batch(schema, embed_dim)
        }).await?;

        let driver_table = open_or_create(&conn, "drivers", placeholder_schema(), |schema| {
            empty_placeholder_batch(schema)
        }).await?;

        let truck_table = open_or_create(&conn, "trucks", placeholder_schema(), |schema| {
            empty_placeholder_batch(schema)
        }).await?;

        let trailer_table = open_or_create(&conn, "trailers", placeholder_schema(), |schema| {
            empty_placeholder_batch(schema)
        }).await?;

        let trip_table = open_or_create(&conn, "trips", placeholder_schema(), |schema| {
            empty_placeholder_batch(schema)
        }).await?;

        let event_table = open_or_create(&conn, "events", event_schema(embed_dim), |schema| {
            empty_event_batch(schema, embed_dim)
        }).await?;

        Ok(Self {
            blob_table,
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

pub fn placeholder_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
    ]))
}

fn empty_placeholder_batch(schema: Arc<Schema>) -> Result<RecordBatch, AppError> {
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
    ])
    .map_err(|e| AppError::Internal(e.to_string()))
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
    async fn test_db_client_creates_all_three_tables() {
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
}
