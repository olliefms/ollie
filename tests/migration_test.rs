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

/// Pre-#268 drivers schema: identical to current minus `current_truck_id`
/// and `current_trailer_ids`.
fn driver_schema_pre_equipment(embed_dim: usize) -> Arc<Schema> {
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

fn driver_pre_equipment_row_batch(schema: Arc<Schema>, embed_dim: usize) -> RecordBatch {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let nulls: Vec<Option<Vec<Option<f32>>>> = vec![None];
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(vec![Some(id.as_str())])),
        Arc::new(StringArray::from(vec![Some("Legacy Driver")])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(StringArray::from(vec![Some("available")])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(
            FixedSizeListArray::from_iter_primitive::<arrow_array::types::Float32Type, _, _>(
                nulls, embed_dim as i32,
            ),
        ),
        Arc::new(Int64Array::from(vec![1_i64])),
        Arc::new(StringArray::from(vec![Some(now.as_str())])),
        Arc::new(StringArray::from(vec![Some(now.as_str())])),
    ]).unwrap()
}

async fn seed_pre_equipment_drivers(path: &str) {
    let conn = lancedb::connect(path).execute().await.unwrap();
    let schema = driver_schema_pre_equipment(EMBED_DIM);
    let batch = driver_pre_equipment_row_batch(schema.clone(), EMBED_DIM);
    let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
    let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
    conn.create_table("drivers", reader).execute().await.unwrap();
}

#[tokio::test]
async fn migration_opens_pre_equipment_drivers_table_and_adds_new_columns() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();

    seed_pre_equipment_drivers(path).await;

    let client = DbClient::new(path, EMBED_DIM).await.expect(
        "DbClient::new must migrate a pre-#268 drivers table without erroring",
    );

    let drivers_schema = client.driver_table.schema().await.unwrap();
    assert!(drivers_schema.field_with_name("current_truck_id").is_ok());
    assert!(drivers_schema.field_with_name("current_trailer_ids").is_ok());
    assert!(
        drivers_schema.field_with_name("blob_ids").is_ok(),
        "post-migration drivers schema missing blob_ids (#279)"
    );
    assert_eq!(client.driver_table.count_rows(None).await.unwrap(), 1);

    // Read pre-existing row via ops layer — defaults must round-trip.
    let (_total, items) = client.list_drivers(None, 10, 0).await.unwrap();
    let id = items[0].id;
    let d = client.get_driver_by_id(id).await.unwrap();
    assert_eq!(d.current_truck_id, None);
    assert_eq!(d.current_trailer_ids, Vec::<Uuid>::new());
    assert_eq!(d.blob_ids, Vec::<Uuid>::new());

    // New equipment write round-trips.
    let new_trailer = Uuid::new_v4();
    let new_truck = Uuid::new_v4();
    let updated = client.update_driver_equipment(
        id,
        Some(Some(new_truck)),
        Some(vec![new_trailer]),
    ).await.unwrap();
    assert_eq!(updated.current_truck_id, Some(new_truck));
    assert_eq!(updated.current_trailer_ids, vec![new_trailer]);

    let refetched = client.get_driver_by_id(id).await.unwrap();
    assert_eq!(refetched.current_truck_id, Some(new_truck));
    assert_eq!(refetched.current_trailer_ids, vec![new_trailer]);
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
    assert!(
        trips_schema.field_with_name("blob_ids").is_ok(),
        "post-migration trips schema missing blob_ids (#279)"
    );

    // Pre-existing row still readable.
    let row_count = client.trip_table.count_rows(None).await.unwrap();
    assert_eq!(row_count, 1, "pre-v16 seed row should survive migration");

    // The pre-existing (pre-blob_ids) row must read back with an empty blob_ids,
    // proving the `'[]'` migration default deserializes cleanly (#279).
    let seed = client.list_trips(None, None, None, None, None).await.unwrap();
    let seed_id = seed[0].id;
    let seed_trip = client.get_trip(seed_id).await.unwrap();
    assert_eq!(seed_trip.blob_ids, Vec::<Uuid>::new());

    // Step 4: insert a fresh trip via ops layer and round-trip the new columns.
    use ollie::models::trip::{TripRecord, TripStatus};
    let new_id = Uuid::new_v4();
    let blob_id = Uuid::new_v4();
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
        blob_ids: vec![blob_id],
        loaded_rate_per_mile: None,
        deadhead_rate_per_mile: None,
        extra_stop_fee: None,
        detention_rate_per_hour: None,
        free_dwell_minutes: None,
        settlement_ref: None,
        pay_period_start: None,
        pay_period_end: None,
        driver_pay_snapshot: None,
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
    assert_eq!(fetched.blob_ids, vec![blob_id], "blob_ids must round-trip post-migration (#279)");

    // Reverse lookup finds the trip by blob.
    assert!(client.any_trip_references_blob(blob_id).await.unwrap());
    assert_eq!(client.trips_referencing_blob(blob_id).await.unwrap(), vec![new_id]);
}

/// Pre-#279 trucks schema: current `truck_schema` minus the appended `blob_ids`.
fn truck_schema_pre_blob_ids(embed_dim: usize) -> Arc<Schema> {
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

async fn seed_pre_blob_ids_trucks(path: &str) {
    let conn = lancedb::connect(path).execute().await.unwrap();
    let schema = truck_schema_pre_blob_ids(EMBED_DIM);
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let nulls: Vec<Option<Vec<Option<f32>>>> = vec![None];
    let batch = RecordBatch::try_new(schema.clone(), vec![
        Arc::new(StringArray::from(vec![Some(id.as_str())])),
        Arc::new(StringArray::from(vec![Some("T-LEGACY")])),
        Arc::new(Int64Array::from(vec![None::<i64>])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(StringArray::from(vec![Some("available")])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(FixedSizeListArray::from_iter_primitive::<arrow_array::types::Float32Type, _, _>(
            nulls, EMBED_DIM as i32,
        )),
        Arc::new(Int64Array::from(vec![1_i64])),
        Arc::new(StringArray::from(vec![Some(now.as_str())])),
        Arc::new(StringArray::from(vec![Some(now.as_str())])),
    ]).unwrap();
    let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
    let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
    conn.create_table("trucks", reader).execute().await.unwrap();
}

/// Pre-#279 trailers schema: current `trailer_schema` minus the appended `blob_ids`.
fn trailer_schema_pre_blob_ids(embed_dim: usize) -> Arc<Schema> {
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

async fn seed_pre_blob_ids_trailers(path: &str) {
    let conn = lancedb::connect(path).execute().await.unwrap();
    let schema = trailer_schema_pre_blob_ids(EMBED_DIM);
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let nulls: Vec<Option<Vec<Option<f32>>>> = vec![None];
    let batch = RecordBatch::try_new(schema.clone(), vec![
        Arc::new(StringArray::from(vec![Some(id.as_str())])),
        Arc::new(StringArray::from(vec![Some("TR-LEGACY")])),
        Arc::new(StringArray::from(vec![Some("fleet")])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(Int64Array::from(vec![None::<i64>])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(Float64Array::from(vec![None::<f64>])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(StringArray::from(vec![Some("available")])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(FixedSizeListArray::from_iter_primitive::<arrow_array::types::Float32Type, _, _>(
            nulls, EMBED_DIM as i32,
        )),
        Arc::new(Int64Array::from(vec![1_i64])),
        Arc::new(StringArray::from(vec![Some(now.as_str())])),
        Arc::new(StringArray::from(vec![Some(now.as_str())])),
    ]).unwrap();
    let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
    let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
    conn.create_table("trailers", reader).execute().await.unwrap();
}

/// #279 added dedicated `open_or_create_truck`/`open_or_create_trailer` migration
/// functions (previously these tables used the generic, non-migrating helper).
/// Seed pre-blob_ids trucks + trailers tables, then assert the `blob_ids` column
/// is added (`'[]'` default), pre-existing rows survive, and writes round-trip.
#[tokio::test]
async fn migration_opens_pre_blob_ids_trucks_and_trailers_and_adds_blob_ids() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();

    seed_pre_blob_ids_trucks(path).await;
    seed_pre_blob_ids_trailers(path).await;

    let client = DbClient::new(path, EMBED_DIM).await.expect(
        "DbClient::new must migrate pre-#279 trucks/trailers tables without erroring. \
         If this fails with a DataFusion CAST parser error, the blob_ids migration \
         is using an Arrow type name where a SQL keyword is required — see AGENTS.md.",
    );

    let trucks_schema = client.truck_table.schema().await.unwrap();
    assert!(trucks_schema.field_with_name("blob_ids").is_ok(),
        "post-migration trucks schema missing blob_ids (#279)");
    let trailers_schema = client.trailer_table.schema().await.unwrap();
    assert!(trailers_schema.field_with_name("blob_ids").is_ok(),
        "post-migration trailers schema missing blob_ids (#279)");

    // Pre-existing rows survive, with blob_ids defaulting to empty.
    let (_t, trucks) = client.list_trucks(None, 10, 0).await.unwrap();
    let truck_id = trucks[0].id;
    assert_eq!(client.get_truck_by_id(truck_id).await.unwrap().blob_ids, Vec::<Uuid>::new());
    let (_t2, trailers) = client.list_trailers(None, None, 10, 0).await.unwrap();
    let trailer_id = trailers[0].id;
    assert_eq!(client.get_trailer_by_id(trailer_id).await.unwrap().blob_ids, Vec::<Uuid>::new());

    // A blob_ids write round-trips and is found by the reverse lookup.
    let blob_id = Uuid::new_v4();
    client.update_truck_metadata(
        truck_id, None, None, None, None, None, None, None, None, Some(vec![blob_id]),
    ).await.unwrap();
    assert_eq!(client.get_truck_by_id(truck_id).await.unwrap().blob_ids, vec![blob_id]);
    assert!(client.any_truck_references_blob(blob_id).await.unwrap());
    assert_eq!(client.trucks_referencing_blob(blob_id).await.unwrap(), vec![truck_id]);

    client.update_trailer_metadata(
        trailer_id, None, None, None, None, None, None, None, None, None, None, None, Some(vec![blob_id]),
    ).await.unwrap();
    assert_eq!(client.get_trailer_by_id(trailer_id).await.unwrap().blob_ids, vec![blob_id]);
    assert!(client.any_trailer_references_blob(blob_id).await.unwrap());
    assert_eq!(client.trailers_referencing_blob(blob_id).await.unwrap(), vec![trailer_id]);
}
