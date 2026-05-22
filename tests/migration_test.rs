// tests/migration_test.rs
//
// Existing-DB migration integration test (#251).
//
// Builds a pre-v1.17 LanceDB fixture programmatically (trips table at v1.15.0
// schema — i.e. missing the v1.16.0-added `total_miles` and `segment_miles`
// columns), populates each table with one row, then opens the same DB path
// with the current `DbClient::new(...)` and asserts:
//
//   1. Every `open_or_create_*` migration path completes without error.
//   2. The post-migration trip schema contains the new columns.
//   3. A fresh trip inserted post-migration round-trips through `trip_ops`
//      including the v1.16.0 columns.
//
// Guards against the recurring CAST-type regression documented in AGENTS.md
// (v1.10.0, v1.13.0, v1.16.0).

use arrow_array::{
    FixedSizeListArray, Float64Array, Int64Array, RecordBatch, RecordBatchIterator,
    RecordBatchReader, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use chrono::Utc;
use ollie::db::DbClient;
use std::sync::Arc;
use tempfile::TempDir;
use uuid::Uuid;

const EMBED_DIM: usize = 4;

/// Pre-v1.16.0 trip schema: identical to the current `trip_schema` minus
/// `total_miles` and `segment_miles`.
fn trip_schema_v15(embed_dim: usize) -> Arc<Schema> {
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
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                embed_dim as i32,
            ),
            true,
        ),
        Field::new("owner_id", DataType::Int64, false),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
        Field::new("load_number", DataType::Utf8, true),
        Field::new("previous_trip_id", DataType::Utf8, true),
        Field::new("deadhead_miles", DataType::Float64, true),
        Field::new("loaded_miles", DataType::Float64, true),
    ]))
}

fn trip_v15_row_batch(schema: Arc<Schema>, embed_dim: usize) -> RecordBatch {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let nulls: Vec<Option<Vec<Option<f32>>>> = vec![None];
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(vec![Some(id.as_str())])),
            Arc::new(StringArray::from(vec![Some("T-2026-9999")])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(Int64Array::from(vec![1_i64])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(StringArray::from(vec![Some("[]")])),
            Arc::new(StringArray::from(vec![Some("planned")])),
            Arc::new(StringArray::from(vec![Some("[]")])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(
                FixedSizeListArray::from_iter_primitive::<arrow_array::types::Float32Type, _, _>(
                    nulls,
                    embed_dim as i32,
                ),
            ),
            Arc::new(Int64Array::from(vec![1_i64])),
            Arc::new(StringArray::from(vec![Some(now.as_str())])),
            Arc::new(StringArray::from(vec![Some(now.as_str())])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(Float64Array::from(vec![None::<f64>])),
            Arc::new(Float64Array::from(vec![None::<f64>])),
        ],
    )
    .unwrap()
}

async fn seed_pre_v16_db(path: &str) {
    let conn = lancedb::connect(path).execute().await.unwrap();
    let schema = trip_schema_v15(EMBED_DIM);
    let batch = trip_v15_row_batch(schema.clone(), EMBED_DIM);
    let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
    let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
    conn.create_table("trips", reader).execute().await.unwrap();
}

#[tokio::test]
async fn migration_opens_pre_v16_trips_table_and_adds_new_columns() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();

    // Step 1: build the v1.15.0 fixture.
    seed_pre_v16_db(path).await;

    // Step 2: open with current DbClient::new — exercises every open_or_create_*
    // path against an existing-row population. Migration must succeed.
    let client = DbClient::new(path, EMBED_DIM).await.expect(
        "DbClient::new must migrate a pre-v1.16.0 trips table without erroring. \
         If this fails with a DataFusion CAST parser error, the migration is \
         using an Arrow type name (e.g. `float64`) where a SQL keyword \
         (`double`) is required — see AGENTS.md.",
    );

    // Step 3: post-migration schema must include the v1.16.0 columns.
    let trips_schema = client.trip_table.schema().await.unwrap();
    assert!(
        trips_schema.field_with_name("total_miles").is_ok(),
        "post-migration trips schema missing total_miles"
    );
    assert!(
        trips_schema.field_with_name("segment_miles").is_ok(),
        "post-migration trips schema missing segment_miles"
    );

    // Pre-existing row still readable.
    let row_count = client.trip_table.count_rows(None).await.unwrap();
    assert_eq!(row_count, 1, "pre-v16 seed row should survive migration");

    // Step 4: insert a fresh trip via ops layer and round-trip the new columns.
    use ollie::models::trip::{TripRecord, TripStatus};
    let new_id = Uuid::new_v4();
    let now = Utc::now();
    let record = TripRecord {
        id: new_id,
        trip_number: "T-2026-0001".into(),
        load_id: None,
        load_number: None,
        previous_trip_id: None,
        deadhead_miles: Some(12.5),
        loaded_miles: Some(100.0),
        total_miles: Some(112.5),
        segment_miles: vec![12.5, 50.0, 50.0],
        sequence: 1,
        driver_id: None,
        truck_id: None,
        trailer_ids: vec![],
        status: TripStatus::Planned,
        stops: vec![],
        notes: None,
        embedding: None,
        owner_id: 1,
        created_at: now,
        updated_at: now,
    };
    client.insert_trip(&record).await.unwrap();

    let fetched = client.get_trip(new_id).await.unwrap();
    assert_eq!(fetched.total_miles, Some(112.5));
    assert_eq!(fetched.segment_miles, vec![12.5, 50.0, 50.0]);
    assert_eq!(fetched.deadhead_miles, Some(12.5));
    assert_eq!(fetched.loaded_miles, Some(100.0));
}
