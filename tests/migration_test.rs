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
    FixedSizeListArray, Float64Array, Int32Array, Int64Array, RecordBatch, RecordBatchIterator,
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

/// Pre-#331 fleet_users schema: current `fleet_user_schema` minus the
/// appended `role` and `extra_scopes` columns.
fn fleet_user_schema_pre_roles() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("email", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
    ]))
}

async fn seed_pre_roles_fleet_users(path: &str) -> Uuid {
    let conn = lancedb::connect(path).execute().await.unwrap();
    let schema = fleet_user_schema_pre_roles();
    let id = Uuid::new_v4();
    let id_str = id.to_string();
    let now = Utc::now().to_rfc3339();
    let batch = RecordBatch::try_new(schema.clone(), vec![
        Arc::new(StringArray::from(vec![Some(id_str.as_str())])),
        Arc::new(StringArray::from(vec![Some("legacy@example.com")])),
        Arc::new(StringArray::from(vec![Some("Legacy Dispatcher")])),
        Arc::new(StringArray::from(vec![Some("active")])),
        Arc::new(StringArray::from(vec![Some(now.as_str())])),
        Arc::new(StringArray::from(vec![Some(now.as_str())])),
    ]).unwrap();
    let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
    let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
    conn.create_table("fleet_users", reader).execute().await.unwrap();
    id
}

/// #331 added `role` + `extra_scopes` columns to the fleet_users table (the
/// permission model's per-user storage). Seed a pre-#331 fleet_users table,
/// then assert the migration adds the columns with the documented defaults
/// (`role='dispatcher'`, `extra_scopes='[]'`) and that the pre-existing row
/// round-trips through the ops layer.
#[tokio::test]
async fn migration_opens_pre_roles_fleet_users_and_adds_role_and_extra_scopes() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();

    let seeded_id = seed_pre_roles_fleet_users(path).await;

    let client = DbClient::new(path, EMBED_DIM).await.expect(
        "DbClient::new must migrate a pre-#331 fleet_users table without erroring. \
         If this fails with a DataFusion CAST parser error, the role/extra_scopes \
         migration is using an Arrow type name (e.g. `Utf8`) where a SQL keyword \
         (`string`) is required — see AGENTS.md.",
    );

    let schema = client.fleet_user_table.schema().await.unwrap();
    assert!(schema.field_with_name("role").is_ok(),
        "post-migration fleet_users schema missing role (#331)");
    assert!(schema.field_with_name("extra_scopes").is_ok(),
        "post-migration fleet_users schema missing extra_scopes (#331)");

    assert_eq!(client.fleet_user_table.count_rows(None).await.unwrap(), 1);

    // Pre-existing row reads back with migration defaults for extra_scopes. Its
    // role column is added as `fleet_user`, but the #331 owner-bootstrap reconcile
    // (run from DbClient::new) then auto-promotes the sole/oldest fleet_user to
    // owner, since an existing install would otherwise have no owner.
    use ollie::models::Role;
    let fetched = client.get_fleet_user_by_id(seeded_id).await.unwrap();
    assert_eq!(fetched.email, "legacy@example.com");
    assert_eq!(fetched.role, Role::Owner);
    assert_eq!(fetched.extra_scopes, Vec::<String>::new());

    // A fresh write carrying role + extra_scopes round-trips.
    use ollie::models::{FleetUserRecord, FleetUserStatus};
    let new_id = Uuid::new_v4();
    let now = Utc::now();
    let record = FleetUserRecord {
        id: new_id,
        email: "owner@example.com".into(),
        name: "New Owner".into(),
        status: FleetUserStatus::Active,
        role: Role::FleetManager,
        extra_scopes: vec!["loads:settle".into(), "loads:invoice".into()],
        created_at: now,
        updated_at: now,
    };
    client.upsert_fleet_user(&record).await.unwrap();
    let refetched = client.get_fleet_user_by_id(new_id).await.unwrap();
    assert_eq!(refetched.role, Role::FleetManager);
    assert_eq!(refetched.extra_scopes, vec!["loads:settle".to_string(), "loads:invoice".to_string()]);
}

/// #331 owner-bootstrap reconcile: an existing install with fleet_users but no
/// owner must auto-promote the OLDEST fleet_user (lowest created_at) on the next
/// DbClient::new, leaving the others as `fleet_user`. Seed three fleet_users with
/// distinct created_at through a first DbClient (whose reconcile is a no-op over
/// the initially-empty table), then re-open over the same data to run reconcile.
#[tokio::test]
async fn reconcile_promotes_oldest_fleet_user_when_no_owner_exists() {
    use ollie::models::{FleetUserRecord, FleetUserStatus, Role};
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();

    let oldest_id;
    {
        let client = DbClient::new(path, EMBED_DIM).await.unwrap();
        let base = Utc::now();
        let mk = |email: &str, mins_ago: i64| FleetUserRecord {
            id: Uuid::new_v4(),
            email: email.into(),
            name: email.into(),
            status: FleetUserStatus::Active,
            role: Role::Dispatcher,
            extra_scopes: Vec::new(),
            created_at: base - chrono::Duration::minutes(mins_ago),
            updated_at: base,
        };
        let oldest = mk("oldest@example.com", 100);
        oldest_id = oldest.id;
        client.insert_fleet_user(&oldest).await.unwrap();
        client.insert_fleet_user(&mk("middle@example.com", 50)).await.unwrap();
        client.insert_fleet_user(&mk("newest@example.com", 10)).await.unwrap();
    }

    // Re-open: reconcile runs and promotes the oldest.
    let client = DbClient::new(path, EMBED_DIM).await.unwrap();
    let users = client.list_fleet_users().await.unwrap();
    assert_eq!(users.len(), 3);
    let owners: Vec<_> = users.iter().filter(|u| u.role == Role::Owner).collect();
    assert_eq!(owners.len(), 1, "exactly one owner after reconcile");
    assert_eq!(owners[0].id, oldest_id, "oldest fleet_user must be promoted");
    for u in &users {
        if u.id != oldest_id {
            assert_eq!(u.role, Role::Dispatcher, "non-oldest must stay fleet_user");
        }
    }

    // Idempotent: a further re-open does not change ownership.
    let client2 = DbClient::new(path, EMBED_DIM).await.unwrap();
    let owners2: Vec<_> = client2.list_fleet_users().await.unwrap()
        .into_iter().filter(|u| u.role == Role::Owner).collect();
    assert_eq!(owners2.len(), 1);
    assert_eq!(owners2[0].id, oldest_id);
}

/// Reconcile is a no-op when an owner already exists: an install that already has
/// an owner (plus older fleet_users) must keep that owner untouched.
#[tokio::test]
async fn reconcile_is_noop_when_owner_already_exists() {
    use ollie::models::{FleetUserRecord, FleetUserStatus, Role};
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();

    let owner_id;
    {
        let client = DbClient::new(path, EMBED_DIM).await.unwrap();
        let base = Utc::now();
        // An OLDER fleet_user plus a newer explicit owner. If reconcile fired it
        // would (wrongly) promote the older fleet_user.
        let older = FleetUserRecord {
            id: Uuid::new_v4(),
            email: "older@example.com".into(),
            name: "Older".into(),
            status: FleetUserStatus::Active,
            role: Role::Dispatcher,
            extra_scopes: Vec::new(),
            created_at: base - chrono::Duration::minutes(100),
            updated_at: base,
        };
        let owner = FleetUserRecord {
            id: Uuid::new_v4(),
            email: "owner@example.com".into(),
            name: "Owner".into(),
            status: FleetUserStatus::Active,
            role: Role::Owner,
            extra_scopes: Vec::new(),
            created_at: base - chrono::Duration::minutes(10),
            updated_at: base,
        };
        owner_id = owner.id;
        client.insert_fleet_user(&older).await.unwrap();
        client.insert_fleet_user(&owner).await.unwrap();
    }

    let client = DbClient::new(path, EMBED_DIM).await.unwrap();
    let owners: Vec<_> = client.list_fleet_users().await.unwrap()
        .into_iter().filter(|u| u.role == Role::Owner).collect();
    assert_eq!(owners.len(), 1, "reconcile must not add a second owner");
    assert_eq!(owners[0].id, owner_id, "existing owner must be unchanged");
}

/// Fresh install (zero fleet_users): reconcile leaves the table empty.
#[tokio::test]
async fn reconcile_noop_on_fresh_install() {
    use ollie::models::Role;
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    let client = DbClient::new(path, EMBED_DIM).await.unwrap();
    let users = client.list_fleet_users().await.unwrap();
    assert!(users.is_empty(), "fresh install has zero fleet_users");
    assert_eq!(client.count_fleet_users().await.unwrap(), 0);
    assert!(!users.iter().any(|u| u.role == Role::Owner));
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

// ── dispatcher -> fleet-user rename migration (#335 follow-up) ──────────────
//
// Seeds a DB with the OLD table names (`dispatchers`, `dispatcher_credentials`
// with a `dispatcher_id` FK, `dispatcher_api_keys`) and a `refresh_tokens` table
// holding one `subject_type="dispatcher"` row and one `"driver"` row, then opens
// `DbClient::new` and asserts the migration:
//   1. renames the three account tables (data preserved, `dispatcher_id` ->
//      `fleet_user_id`) and drops the old ones,
//   2. rewrites the dispatcher refresh-token subject_type to "fleet_user" while
//      leaving the driver row untouched,
//   3. leaves the read path working through the renamed db-ops.

async fn create_one(conn: &lancedb::Connection, name: &str, schema: Arc<Schema>, batch: RecordBatch) {
    let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
    let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
    conn.create_table(name, reader).execute().await.unwrap();
}

async fn seed_dispatcher_named_db(path: &str, user_id: &str) {
    let conn = lancedb::connect(path).execute().await.unwrap();
    let now = Utc::now().to_rfc3339();

    // dispatchers (same columns as fleet_users)
    let disp_schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("email", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("role", DataType::Utf8, false),
        Field::new("extra_scopes", DataType::Utf8, false),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
    ]));
    let disp_batch = RecordBatch::try_new(disp_schema.clone(), vec![
        Arc::new(StringArray::from(vec![Some(user_id)])),
        Arc::new(StringArray::from(vec![Some("legacy@example.com")])),
        Arc::new(StringArray::from(vec![Some("Legacy User")])),
        Arc::new(StringArray::from(vec![Some("active")])),
        Arc::new(StringArray::from(vec![Some("dispatcher")])),
        Arc::new(StringArray::from(vec![Some("[]")])),
        Arc::new(StringArray::from(vec![Some(now.as_str())])),
        Arc::new(StringArray::from(vec![Some(now.as_str())])),
    ]).unwrap();
    create_one(&conn, "dispatchers", disp_schema, disp_batch).await;

    // dispatcher_credentials (FK column named dispatcher_id)
    let cred_schema = Arc::new(Schema::new(vec![
        Field::new("dispatcher_id", DataType::Utf8, false),
        Field::new("password_hash", DataType::Utf8, false),
        Field::new("token_version", DataType::Int64, false),
        Field::new("failed_attempts", DataType::Int32, false),
        Field::new("locked_until", DataType::Utf8, true),
        Field::new("updated_at", DataType::Utf8, false),
    ]));
    let cred_batch = RecordBatch::try_new(cred_schema.clone(), vec![
        Arc::new(StringArray::from(vec![Some(user_id)])),
        Arc::new(StringArray::from(vec![Some("$2b$12$abcdefghijklmnopqrstuv")])),
        Arc::new(Int64Array::from(vec![0_i64])),
        Arc::new(Int32Array::from(vec![0_i32])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(StringArray::from(vec![Some(now.as_str())])),
    ]).unwrap();
    create_one(&conn, "dispatcher_credentials", cred_schema, cred_batch).await;

    // dispatcher_api_keys (FK column named dispatcher_id)
    let key_schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("dispatcher_id", DataType::Utf8, false),
        Field::new("label", DataType::Utf8, false),
        Field::new("key_hash", DataType::Utf8, false),
        Field::new("key_prefix", DataType::Utf8, false),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("expires_at", DataType::Utf8, false),
        Field::new("revoked_at", DataType::Utf8, true),
        Field::new("last_used_at", DataType::Utf8, true),
    ]));
    let future = (Utc::now() + chrono::Duration::days(365)).to_rfc3339();
    let key_batch = RecordBatch::try_new(key_schema.clone(), vec![
        Arc::new(StringArray::from(vec![Some(Uuid::new_v4().to_string().as_str())])),
        Arc::new(StringArray::from(vec![Some(user_id)])),
        Arc::new(StringArray::from(vec![Some("legacy key")])),
        Arc::new(StringArray::from(vec![Some("hash")])),
        Arc::new(StringArray::from(vec![Some("ollie_ab")])),
        Arc::new(StringArray::from(vec![Some(now.as_str())])),
        Arc::new(StringArray::from(vec![Some(future.as_str())])),
        Arc::new(StringArray::from(vec![None::<&str>])),
        Arc::new(StringArray::from(vec![None::<&str>])),
    ]).unwrap();
    create_one(&conn, "dispatcher_api_keys", key_schema, key_batch).await;

    // refresh_tokens: one dispatcher row + one driver row
    let rt_schema = Arc::new(Schema::new(vec![
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
    ]));
    let rt_batch = RecordBatch::try_new(rt_schema.clone(), vec![
        Arc::new(StringArray::from(vec![Some(Uuid::new_v4().to_string().as_str()), Some(Uuid::new_v4().to_string().as_str())])),
        Arc::new(StringArray::from(vec![Some("h1"), Some("h2")])),
        Arc::new(StringArray::from(vec![Some("dispatcher"), Some("driver")])),
        Arc::new(StringArray::from(vec![Some(user_id), Some(Uuid::new_v4().to_string().as_str())])),
        Arc::new(StringArray::from(vec![None::<&str>, None::<&str>])),
        Arc::new(StringArray::from(vec![Some(Uuid::new_v4().to_string().as_str()), Some(Uuid::new_v4().to_string().as_str())])),
        Arc::new(Int64Array::from(vec![0_i64, 0_i64])),
        Arc::new(StringArray::from(vec![Some(now.as_str()), Some(now.as_str())])),
        Arc::new(StringArray::from(vec![Some(future.as_str()), Some(future.as_str())])),
        Arc::new(StringArray::from(vec![None::<&str>, None::<&str>])),
        Arc::new(StringArray::from(vec![None::<&str>, None::<&str>])),
        Arc::new(StringArray::from(vec![None::<&str>, None::<&str>])),
    ]).unwrap();
    create_one(&conn, "refresh_tokens", rt_schema, rt_batch).await;
}

async fn subject_types(conn: &lancedb::Connection, table: &str) -> Vec<String> {
    use arrow_array::Array;
    use futures::TryStreamExt;
    use lancedb::query::ExecutableQuery;
    let t = conn.open_table(table).execute().await.unwrap();
    let batches: Vec<RecordBatch> = t.query().execute().await.unwrap().try_collect().await.unwrap();
    let mut out = Vec::new();
    for b in &batches {
        let idx = b.schema().index_of("subject_type").unwrap();
        let col = b.column(idx).as_any().downcast_ref::<StringArray>().unwrap();
        for i in 0..col.len() {
            out.push(col.value(i).to_string());
        }
    }
    out
}

#[tokio::test]
async fn migration_renames_dispatcher_tables_to_fleet_user() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_str().unwrap();
    let user_id = Uuid::new_v4();
    seed_dispatcher_named_db(path, &user_id.to_string()).await;

    let client = DbClient::new(path, EMBED_DIM).await
        .expect("DbClient::new must run the dispatcher->fleet_user migration cleanly");

    // 1. Account data survived into the new tables, read path intact.
    let user = client.get_fleet_user_by_id(user_id).await
        .expect("migrated fleet user must be readable by id");
    assert_eq!(user.email, "legacy@example.com");
    assert!(client.get_fleet_user_credentials(user_id).await.unwrap().is_some(),
        "migrated credentials (dispatcher_id -> fleet_user_id) must be readable");
    assert_eq!(client.count_active_fleet_user_api_keys(user_id).await.unwrap(), 1,
        "migrated api key must survive");

    // 2. Old tables dropped, new tables present.
    let conn = lancedb::connect(path).execute().await.unwrap();
    let names = conn.table_names().execute().await.unwrap();
    for old in ["dispatchers", "dispatcher_credentials", "dispatcher_api_keys"] {
        assert!(!names.iter().any(|n| n == old), "old table {old} must be dropped");
    }
    for new in ["fleet_users", "fleet_user_credentials", "fleet_user_api_keys"] {
        assert!(names.iter().any(|n| n == new), "new table {new} must exist");
    }

    // 3. Refresh-token subject_type rewritten for dispatcher rows only.
    let mut subs = subject_types(&conn, "refresh_tokens").await;
    subs.sort();
    assert_eq!(subs, vec!["driver".to_string(), "fleet_user".to_string()],
        "dispatcher subject_type -> fleet_user; driver row untouched");
}

// --- Facilities `archived` soft-delete column migration (Phase 3) ---

/// Pre-archived facilities schema: the current `facility_schema` minus the
/// `archived` column appended at the end.
fn facility_schema_pre_archived(embed_dim: usize) -> Arc<Schema> {
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
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                embed_dim as i32,
            ),
            true,
        ),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
    ]))
}

async fn seed_pre_archived_facilities(path: &str) {
    let conn = lancedb::connect(path).execute().await.unwrap();
    let schema = facility_schema_pre_archived(EMBED_DIM);
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let nulls: Vec<Option<Vec<Option<f32>>>> = vec![None];
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(vec![Some(id.as_str())])),
            Arc::new(Int64Array::from(vec![0_i64])),
            Arc::new(StringArray::from(vec![Some("Legacy Facility")])),
            Arc::new(StringArray::from(vec![Some("Memphis, TN")])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(Float64Array::from(vec![None::<f64>])),
            Arc::new(Float64Array::from(vec![None::<f64>])),
            Arc::new(StringArray::from(vec![Some("pending")])),
            Arc::new(StringArray::from(vec![Some("[]")])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(StringArray::from(vec![Some("[]")])),
            Arc::new(StringArray::from(vec![Some("[]")])),
            Arc::new(Float64Array::from(vec![None::<f64>])),
            Arc::new(Int64Array::from(vec![0_i64])),
            Arc::new(Int64Array::from(vec![0_i64])),
            Arc::new(
                FixedSizeListArray::from_iter_primitive::<arrow_array::types::Float32Type, _, _>(
                    nulls,
                    EMBED_DIM as i32,
                ),
            ),
            Arc::new(StringArray::from(vec![Some(now.as_str())])),
            Arc::new(StringArray::from(vec![Some(now.as_str())])),
        ],
    )
    .unwrap();
    let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
    let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
    conn.create_table("facilities", reader).execute().await.unwrap();
}

#[tokio::test]
async fn migration_opens_pre_archived_facilities_table_and_adds_archived_column() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();

    seed_pre_archived_facilities(path).await;

    let client = DbClient::new(path, EMBED_DIM).await.expect(
        "DbClient::new must migrate a pre-archived facilities table without erroring",
    );

    let schema = client.facility_table.schema().await.unwrap();
    assert!(
        schema.field_with_name("archived").is_ok(),
        "post-migration facilities schema missing archived"
    );
    assert_eq!(client.facility_table.count_rows(None).await.unwrap(), 1);

    // Pre-existing row defaults to archived = false and stays in active lists.
    let (total, items) = client.list_facilities(None, &[], 10, 0, false).await.unwrap();
    assert_eq!(total, 1);
    let id = items[0].id;
    let fac = client.get_facility_by_id(id).await.unwrap();
    assert!(!fac.archived, "migrated row must default to archived = false");

    // Soft archive round-trips and drops the row from the active list.
    let archived = client.set_facility_archived(id, true).await.unwrap();
    assert!(archived.archived);
    let (active_total, _) = client.list_facilities(None, &[], 10, 0, false).await.unwrap();
    assert_eq!(active_total, 0, "archived facility must drop out of active list");
    // Still fetchable by id (for detail / reactivate).
    assert!(client.get_facility_by_id(id).await.unwrap().archived);

    // Reactivate brings it back.
    client.set_facility_archived(id, false).await.unwrap();
    let (back, _) = client.list_facilities(None, &[], 10, 0, false).await.unwrap();
    assert_eq!(back, 1, "reactivated facility must return to the active list");
}
