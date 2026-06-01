// src/db/mod.rs
pub mod blob_ops;
pub mod dispatcher_api_key_ops;
pub mod oauth_client_ops;
pub mod refresh_token_ops;
pub mod authorization_code_ops;
pub mod dispatcher_ops;
pub mod driver_credentials_ops;
pub mod driver_ops;
pub mod event_ops;
pub mod facility_ops;
pub mod load_ops;
pub mod terminal_ops;
pub mod trailer_ops;
pub mod trip_ops;
pub mod truck_ops;

use crate::error::AppError;
use arrow_array::{
    BooleanArray, FixedSizeListArray, Float64Array, Int32Array, Int64Array, RecordBatch,
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
    pub refresh_token_table: Table,
    pub oauth_client_table: Table,
    pub authorization_code_table: Table,
    pub driver_credentials_table: Table,
    pub driver_passkey_credentials_table: Table,
    pub driver_table: Table,
    pub event_table: Table,
    pub facility_table: Table,
    pub load_table: Table,
    pub terminal_table: Table,
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

        let terminal_table = open_or_create_terminal(&conn).await?;

        let driver_table = open_or_create_driver(&conn, embed_dim).await?;

        // Backfill any driver rows with NULL terminal_id -> default terminal id.
        // terminal_table must be open before driver_table for this to work.
        if let Err(e) = backfill_driver_terminals(&terminal_table, &driver_table).await {
            tracing::warn!("driver terminal backfill skipped: {e}");
        }

        let truck_table = open_or_create_truck(&conn, embed_dim).await?;

        let trailer_table = open_or_create_trailer(&conn, embed_dim).await?;

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

        let dispatcher_table = open_or_create_dispatcher(&conn).await?;

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

        let refresh_token_table = open_or_create(
            &conn,
            "refresh_tokens",
            refresh_token_schema(),
            empty_refresh_token_batch,
        ).await?;

        let oauth_client_table = open_or_create(
            &conn,
            "oauth_clients",
            oauth_client_schema(),
            empty_oauth_client_batch,
        ).await?;

        let authorization_code_table = open_or_create(
            &conn,
            "authorization_codes",
            authorization_code_schema(),
            empty_authorization_code_batch,
        ).await?;

        Ok(Self {
            blob_table,
            dispatcher_table,
            dispatcher_credentials_table,
            dispatcher_api_key_table,
            refresh_token_table,
            oauth_client_table,
            authorization_code_table,
            driver_credentials_table,
            driver_passkey_credentials_table,
            driver_table,
            event_table,
            facility_table,
            load_table,
            terminal_table,
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

async fn open_or_create_dispatcher(conn: &lancedb::Connection) -> Result<Table, AppError> {
    let schema = dispatcher_schema();
    match conn.open_table("dispatchers").execute().await {
        Err(_) => {
            let batch = empty_dispatcher_batch(schema.clone())?;
            let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
            let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
            conn.create_table("dispatchers", reader).execute().await
                .map_err(|e| AppError::Internal(e.to_string()))
        }
        Ok(table) => {
            let existing = table.schema().await.map_err(|e| AppError::Internal(e.to_string()))?;
            let mut transforms: Vec<(String, String)> = Vec::new();
            // SQL keyword type `string`, never the Arrow name `Utf8` — see AGENTS.md.
            if existing.field_with_name("role").is_err() {
                transforms.push(("role".into(), "CAST('dispatcher' AS string)".into()));
            }
            if existing.field_with_name("extra_scopes").is_err() {
                transforms.push(("extra_scopes".into(), "CAST('[]' AS string)".into()));
            }
            if !transforms.is_empty() {
                tracing::info!("migrating dispatchers table: adding {} column(s)", transforms.len());
                table.add_columns(NewColumnTransform::SqlExpressions(transforms), None).await
                    .map_err(|e| AppError::Internal(format!("dispatcher schema migration failed: {e}")))?;
            }
            Ok(table)
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
            if existing.field_with_name("blob_ids").is_err() {
                transforms.push(("blob_ids".into(), "'[]'".into()));
            }
            for col in ["loaded_rate_per_mile", "deadhead_rate_per_mile",
                        "extra_stop_fee", "detention_rate_per_hour"] {
                if existing.field_with_name(col).is_err() {
                    transforms.push((col.into(), "CAST(NULL AS double)".into()));
                }
            }
            if existing.field_with_name("free_dwell_minutes").is_err() {
                transforms.push(("free_dwell_minutes".into(), "CAST(NULL AS bigint)".into()));
            }
            for col in ["settlement_ref", "pay_period_start", "pay_period_end", "driver_pay_snapshot"] {
                if existing.field_with_name(col).is_err() {
                    transforms.push((col.into(), "CAST(NULL AS string)".into()));
                }
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

pub fn terminal_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("address", DataType::Utf8, true),
        Field::new("timezone", DataType::Utf8, false),
        Field::new("is_default", DataType::Boolean, false),
        Field::new("loaded_rate_per_mile", DataType::Float64, false),
        Field::new("deadhead_rate_per_mile", DataType::Float64, false),
        Field::new("extra_stop_fee", DataType::Float64, false),
        Field::new("detention_rate_per_hour", DataType::Float64, false),
        Field::new("free_dwell_minutes", DataType::Int64, false),
        Field::new("owner_id", DataType::Int64, false),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
    ]))
}

async fn open_or_create_terminal(conn: &lancedb::Connection) -> Result<Table, AppError> {
    let schema = terminal_schema();
    match conn.open_table("terminals").execute().await {
        Ok(table) => Ok(table),
        Err(_) => {
            let tz = std::env::var("TERMINAL_TIMEZONE")
                .unwrap_or_else(|_| "America/New_York".to_string());
            let free_dwell: i64 = std::env::var("OLLIE_FREE_DWELL_MINUTES")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(120);
            let now = chrono::Utc::now().to_rfc3339();
            let id = uuid::Uuid::new_v4().to_string();
            let batch = RecordBatch::try_new(schema.clone(), vec![
                Arc::new(StringArray::from(vec![id.as_str()])),
                Arc::new(StringArray::from(vec!["Default"])),
                Arc::new(StringArray::from(vec![None::<&str>])),
                Arc::new(StringArray::from(vec![tz.as_str()])),
                Arc::new(BooleanArray::from(vec![true])),
                Arc::new(Float64Array::from(vec![0.0_f64])),
                Arc::new(Float64Array::from(vec![0.0_f64])),
                Arc::new(Float64Array::from(vec![0.0_f64])),
                Arc::new(Float64Array::from(vec![0.0_f64])),
                Arc::new(Int64Array::from(vec![free_dwell])),
                Arc::new(Int64Array::from(vec![0_i64])),
                Arc::new(StringArray::from(vec![now.as_str()])),
                Arc::new(StringArray::from(vec![now.as_str()])),
            ]).map_err(|e| AppError::Internal(e.to_string()))?;
            let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
            let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
            conn.create_table("terminals", reader).execute().await
                .map_err(|e| AppError::Internal(e.to_string()))
        }
    }
}

/// Backfill driver rows that have a NULL terminal_id by assigning them the
/// default terminal's id. Uses read-modify-upsert (merge_insert) to avoid
/// the unverified `.update()` API.
async fn backfill_driver_terminals(
    terminal_table: &Table,
    driver_table: &Table,
) -> Result<(), AppError> {
    use arrow_array::Array;
    use futures::TryStreamExt;
    use lancedb::query::ExecutableQuery;

    // Find the default terminal id.
    let terminal_batches: Vec<RecordBatch> = terminal_table.query().execute().await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .try_collect().await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let mut default_terminal_id: Option<String> = None;
    'outer: for batch in &terminal_batches {
        let id_col = batch.column_by_name("id")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let default_col = batch.column_by_name("is_default")
            .and_then(|c| c.as_any().downcast_ref::<arrow_array::BooleanArray>());
        if let (Some(ids), Some(defaults)) = (id_col, default_col) {
            for i in 0..batch.num_rows() {
                if defaults.value(i) {
                    default_terminal_id = Some(ids.value(i).to_string());
                    break 'outer;
                }
            }
        }
    }

    let Some(tid) = default_terminal_id else {
        return Err(AppError::Internal("no default terminal found for backfill".into()));
    };

    // Collect driver batches.
    let driver_batches: Vec<RecordBatch> = driver_table.query().execute().await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .try_collect().await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Find rows that need backfill.
    let mut needs_backfill: Vec<String> = Vec::new(); // list of driver ids
    for batch in &driver_batches {
        let id_col = batch.column_by_name("id")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let terminal_col = batch.column_by_name("terminal_id")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        if let Some(ids) = id_col {
            for i in 0..batch.num_rows() {
                let needs = match terminal_col {
                    Some(tc) => tc.is_null(i) || tc.value(i).is_empty(),
                    None => true,
                };
                if needs {
                    needs_backfill.push(ids.value(i).to_string());
                }
            }
        }
    }

    if needs_backfill.is_empty() {
        return Ok(());
    }

    tracing::info!("backfilling terminal_id for {} driver(s)", needs_backfill.len());

    // We need the full driver schema to do a proper upsert.
    // We re-read each row and re-write it with the terminal_id set.
    // To avoid depending on driver_ops (circular module issue), we do a minimal
    // column-level patch: update only the terminal_id column in the batch.
    // Since driver_ops isn't available here, we use a DataFusion SQL UPDATE-style
    // approach via merge_insert on a per-driver basis.
    //
    // Actually: we build a minimal RecordBatch for each driver that only has
    // id + terminal_id columns... but merge_insert requires ALL schema columns.
    //
    // Simplest approach: scan the full batch, rebuild it with terminal_id patched.
    // We do this batch-by-batch to keep memory usage low.
    for batch in &driver_batches {
        let id_col = match batch.column_by_name("id")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>()) {
            Some(c) => c,
            None => continue,
        };
        // Collect row indices that need backfill in this batch.
        let terminal_col = batch.column_by_name("terminal_id")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());
        let rows_to_fix: Vec<usize> = (0..batch.num_rows()).filter(|&i| {
            let needs = match terminal_col {
                Some(tc) => tc.is_null(i) || tc.value(i).is_empty(),
                None => true,
            };
            needs && needs_backfill.contains(&id_col.value(i).to_string())
        }).collect();

        if rows_to_fix.is_empty() { continue; }

        // Build a new schema-matching batch with patched terminal_id.
        // We replace the terminal_id column; all other columns are taken from the original.
        let schema = batch.schema();
        let terminal_idx = schema.index_of("terminal_id").ok();

        let mut new_cols: Vec<Arc<dyn arrow_array::Array>> = Vec::new();
        for col_idx in 0..batch.num_columns() {
            if Some(col_idx) == terminal_idx {
                // Rebuild this column with the default terminal id patched in.
                let orig = batch.column(col_idx).as_any().downcast_ref::<StringArray>().unwrap();
                let patched: Vec<Option<&str>> = (0..batch.num_rows()).map(|i| {
                    if rows_to_fix.contains(&i) {
                        Some(tid.as_str())
                    } else if orig.is_null(i) {
                        None
                    } else {
                        Some(orig.value(i))
                    }
                }).collect();
                new_cols.push(Arc::new(StringArray::from(patched)));
            } else {
                new_cols.push(batch.column(col_idx).clone());
            }
        }

        let patched_batch = RecordBatch::try_new(schema.clone(), new_cols)
            .map_err(|e| AppError::Internal(e.to_string()))?;

        let iter = RecordBatchIterator::new(vec![Ok(patched_batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = driver_table.merge_insert(&["id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await
            .map_err(|e| AppError::Internal(e.to_string()))?;
    }

    Ok(())
}

async fn open_or_create_driver(conn: &lancedb::Connection, embed_dim: usize) -> Result<Table, AppError> {
    let schema = driver_schema(embed_dim);
    match conn.open_table("drivers").execute().await {
        Err(_) => {
            let batch = empty_driver_batch(schema.clone(), embed_dim)?;
            let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
            let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
            conn.create_table("drivers", reader).execute().await
                .map_err(|e| AppError::Internal(e.to_string()))
        }
        Ok(table) => {
            let existing = table.schema().await.map_err(|e| AppError::Internal(e.to_string()))?;
            let mut transforms: Vec<(String, String)> = Vec::new();
            if existing.field_with_name("current_truck_id").is_err() {
                transforms.push(("current_truck_id".into(), "CAST(NULL AS string)".into()));
            }
            if existing.field_with_name("current_trailer_ids").is_err() {
                transforms.push(("current_trailer_ids".into(), "'[]'".into()));
            }
            if existing.field_with_name("blob_ids").is_err() {
                transforms.push(("blob_ids".into(), "'[]'".into()));
            }
            if existing.field_with_name("terminal_id").is_err() {
                transforms.push(("terminal_id".into(), "CAST(NULL AS string)".into()));
            }
            for col in ["loaded_rate_per_mile", "deadhead_rate_per_mile",
                        "extra_stop_fee", "detention_rate_per_hour"] {
                if existing.field_with_name(col).is_err() {
                    transforms.push((col.into(), "CAST(NULL AS double)".into()));
                }
            }
            if existing.field_with_name("free_dwell_minutes").is_err() {
                transforms.push(("free_dwell_minutes".into(), "CAST(NULL AS bigint)".into()));
            }
            if !transforms.is_empty() {
                tracing::info!("migrating drivers table: adding {} column(s)", transforms.len());
                table.add_columns(NewColumnTransform::SqlExpressions(transforms), None).await
                    .map_err(|e| AppError::Internal(format!("driver schema migration failed: {e}")))?;
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

async fn open_or_create_truck(conn: &lancedb::Connection, embed_dim: usize) -> Result<Table, AppError> {
    let schema = truck_schema(embed_dim);
    match conn.open_table("trucks").execute().await {
        Err(_) => {
            let batch = empty_truck_batch(schema.clone(), embed_dim)?;
            let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
            let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
            conn.create_table("trucks", reader).execute().await
                .map_err(|e| AppError::Internal(e.to_string()))
        }
        Ok(table) => {
            let existing = table.schema().await.map_err(|e| AppError::Internal(e.to_string()))?;
            let mut transforms: Vec<(String, String)> = Vec::new();
            if existing.field_with_name("blob_ids").is_err() {
                transforms.push(("blob_ids".into(), "'[]'".into()));
            }
            if !transforms.is_empty() {
                tracing::info!("migrating trucks table: adding {} column(s)", transforms.len());
                table.add_columns(NewColumnTransform::SqlExpressions(transforms), None).await
                    .map_err(|e| AppError::Internal(format!("truck schema migration failed: {e}")))?;
            }
            Ok(table)
        }
    }
}

async fn open_or_create_trailer(conn: &lancedb::Connection, embed_dim: usize) -> Result<Table, AppError> {
    let schema = trailer_schema(embed_dim);
    match conn.open_table("trailers").execute().await {
        Err(_) => {
            let batch = empty_trailer_batch(schema.clone(), embed_dim)?;
            let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
            let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
            conn.create_table("trailers", reader).execute().await
                .map_err(|e| AppError::Internal(e.to_string()))
        }
        Ok(table) => {
            let existing = table.schema().await.map_err(|e| AppError::Internal(e.to_string()))?;
            let mut transforms: Vec<(String, String)> = Vec::new();
            if existing.field_with_name("blob_ids").is_err() {
                transforms.push(("blob_ids".into(), "'[]'".into()));
            }
            if !transforms.is_empty() {
                tracing::info!("migrating trailers table: adding {} column(s)", transforms.len());
                table.add_columns(NewColumnTransform::SqlExpressions(transforms), None).await
                    .map_err(|e| AppError::Internal(format!("trailer schema migration failed: {e}")))?;
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
        Field::new("current_truck_id", DataType::Utf8, true),
        Field::new("current_trailer_ids", DataType::Utf8, false),
        Field::new("blob_ids", DataType::Utf8, false),
        Field::new("terminal_id", DataType::Utf8, true),
        Field::new("loaded_rate_per_mile", DataType::Float64, true),
        Field::new("deadhead_rate_per_mile", DataType::Float64, true),
        Field::new("extra_stop_fee", DataType::Float64, true),
        Field::new("detention_rate_per_hour", DataType::Float64, true),
        Field::new("free_dwell_minutes", DataType::Int64, true),
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
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // current_truck_id
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // current_trailer_ids
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // blob_ids
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // terminal_id
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),  // loaded_rate_per_mile
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),  // deadhead_rate_per_mile
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),  // extra_stop_fee
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),  // detention_rate_per_hour
        Arc::new(Int64Array::from(Vec::<Option<i64>>::new())),    // free_dwell_minutes
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
        Field::new("blob_ids", DataType::Utf8, false),
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
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // blob_ids
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
        Field::new("blob_ids", DataType::Utf8, false),
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
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // blob_ids
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
        Field::new("blob_ids", DataType::Utf8, false),
        Field::new("loaded_rate_per_mile", DataType::Float64, true),
        Field::new("deadhead_rate_per_mile", DataType::Float64, true),
        Field::new("extra_stop_fee", DataType::Float64, true),
        Field::new("detention_rate_per_hour", DataType::Float64, true),
        Field::new("free_dwell_minutes", DataType::Int64, true),
        Field::new("settlement_ref", DataType::Utf8, true),
        Field::new("pay_period_start", DataType::Utf8, true),
        Field::new("pay_period_end", DataType::Utf8, true),
        Field::new("driver_pay_snapshot", DataType::Utf8, true),
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
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // blob_ids
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),  // loaded_rate_per_mile
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),  // deadhead_rate_per_mile
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),  // extra_stop_fee
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),  // detention_rate_per_hour
        Arc::new(Int64Array::from(Vec::<Option<i64>>::new())),    // free_dwell_minutes
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // settlement_ref
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // pay_period_start
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // pay_period_end
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // driver_pay_snapshot
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
        Field::new("role", DataType::Utf8, false),
        Field::new("extra_scopes", DataType::Utf8, false),
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

pub fn oauth_client_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("client_name", DataType::Utf8, true),
        Field::new("redirect_uris", DataType::Utf8, false),
        Field::new("created_at", DataType::Utf8, false),
    ]))
}

fn empty_oauth_client_batch(schema: Arc<Schema>) -> Result<RecordBatch, AppError> {
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

pub fn authorization_code_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("code_hash", DataType::Utf8, false),
        Field::new("client_id", DataType::Utf8, false),
        Field::new("redirect_uri", DataType::Utf8, false),
        Field::new("code_challenge", DataType::Utf8, false),
        Field::new("subject_type", DataType::Utf8, false),
        Field::new("subject_id", DataType::Utf8, false),
        Field::new("resource", DataType::Utf8, false),
        Field::new("scope", DataType::Utf8, true),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("expires_at", DataType::Utf8, false),
        Field::new("consumed_at", DataType::Utf8, true),
    ]))
}

fn empty_authorization_code_batch(schema: Arc<Schema>) -> Result<RecordBatch, AppError> {
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
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

pub fn refresh_token_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("token_hash", DataType::Utf8, false),
        Field::new("subject_type", DataType::Utf8, false),
        Field::new("subject_id", DataType::Utf8, false),
        Field::new("client_id", DataType::Utf8, true),
        Field::new("family_id", DataType::Utf8, false),
        Field::new("token_version", DataType::Int64, false),
        Field::new("issued_at", DataType::Utf8, false),
        Field::new("expires_at", DataType::Utf8, false),
        Field::new("consumed_at", DataType::Utf8, true),
        Field::new("revoked_at", DataType::Utf8, true),
        Field::new("last_used_at", DataType::Utf8, true),
    ]))
}

fn empty_refresh_token_batch(schema: Arc<Schema>) -> Result<RecordBatch, AppError> {
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(Int64Array::from(Vec::<i64>::new())),
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
