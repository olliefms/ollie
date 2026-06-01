// tests/terminals_pay_settlement_test.rs
//
// Integration tests for dispatcher terminal CRUD (#185).

use axum::http::header;
use axum_test::TestServer;
use ollie::{
    ai::OllamaClient,
    api,
    config::Config,
    db::DbClient,
    storage::BlobStore,
    AppState,
};
use std::sync::Arc;
use tempfile::TempDir;
use webauthn_rs::prelude::{Url, WebauthnBuilder};

async fn test_server() -> (TestServer, TempDir, TempDir) {
    let (server, _db, blob_dir, db_dir) = test_server_with_db().await;
    (server, blob_dir, db_dir)
}

async fn test_server_with_db() -> (TestServer, Arc<DbClient>, TempDir, TempDir) {
    let blob_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    std::env::set_var("DRIVER_JWT_SECRET", "test-driver-jwt-secret-that-is-long-enough");
    std::env::set_var("DRIVER_RP_ID", "localhost");
    std::env::set_var("DRIVER_RP_ORIGIN", "http://localhost:3000");
    std::env::set_var("DISPATCHER_JWT_SECRET", "test-dispatcher-secret-must-be-32b");

    let config = Arc::new(Config::from_env().unwrap());
    let db = Arc::new(DbClient::new(db_dir.path().to_str().unwrap(), 4).await.unwrap());
    let store = Arc::new(BlobStore::new(blob_dir.path().to_str().unwrap()));
    let ai = Arc::new(OllamaClient::new(
        "http://localhost:11434", "nomic-embed-text", "llama3.2", "moondream",
    ));
    let geocoding = Arc::new(ollie::geocoding::GeocodingClient::new());
    let ors = Arc::new(ollie::routing::RoutingClient::new(""));

    let (pipeline_tx, _rx) = async_channel::bounded(100);
    let (geocoding_tx, _grx) = async_channel::bounded(100);
    let (routing_tx, _rrx) = async_channel::bounded(100);

    let rp_origin = Url::parse("http://localhost:3000").unwrap();
    let webauthn = Arc::new(
        WebauthnBuilder::new("localhost", &rp_origin)
            .unwrap()
            .build()
            .unwrap(),
    );
    let auth_challenge_store = Arc::new(dashmap::DashMap::new());
    let reg_challenge_store = Arc::new(dashmap::DashMap::new());

    let state = AppState {
        db, store, ai, geocoding, ors,
        pipeline_tx, geocoding_tx, routing_tx, config,
        webauthn, auth_challenge_store, reg_challenge_store,
    };
    let db_handle = state.db.clone();
    let server = TestServer::new(api::router(state)).unwrap();
    (server, db_handle, blob_dir, db_dir)
}

const OWNER_EMAIL: &str = "owner@example.com";
const OWNER_PASSWORD: &str = "owner-password-123";

/// First-run owner bootstrap (idempotent), returning an owner JWT.
async fn setup_owner(server: &TestServer) -> String {
    let resp = server.post("/dispatch/setup")
        .json(&serde_json::json!({
            "email": OWNER_EMAIL, "name": "Owner", "password": OWNER_PASSWORD,
        }))
        .await;
    if resp.status_code() == 200 {
        return resp.json::<serde_json::Value>()["token"].as_str().unwrap().to_string();
    }
    let login = server.post("/dispatch/auth/login")
        .json(&serde_json::json!({ "email": OWNER_EMAIL, "password": OWNER_PASSWORD }))
        .await;
    assert_eq!(login.status_code(), 200, "owner login failed");
    login.json::<serde_json::Value>()["token"].as_str().unwrap().to_string()
}

/// Create a dispatcher account and log in, returning a JWT. Provisioned as
/// `owner` so these terminal/pay/settlement tests (which write terminals and
/// settle trips) pass scope enforcement (#331).
async fn dispatcher_login(server: &TestServer, email: &str, password: &str) -> String {
    // Bootstrap the first-run owner, then create the named user via the users surface.
    // `fleet_manager` is operationally identical to owner (effective scope
    // `["*"]`) but creatable via the users surface (owner is not).
    let owner = setup_owner(server).await;
    server.post("/dispatch/api/v1/users")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner}"))
        .json(&serde_json::json!({
            "email": email,
            "name": "Test Dispatcher",
            "password": password,
            "role": "fleet_manager",
        }))
        .await;

    // Login and return JWT
    let resp = server.post("/dispatch/auth/login")
        .json(&serde_json::json!({ "email": email, "password": password }))
        .await;
    assert_eq!(resp.status_code(), 200, "dispatcher login failed");
    resp.json::<serde_json::Value>()["token"].as_str().unwrap().to_string()
}

// (a) POST create terminal → 201, body.id present
#[tokio::test]
async fn test_create_terminal_returns_201() {
    let (server, _b, _d) = test_server().await;
    let token = dispatcher_login(&server, "term1@example.com", "pw-term1").await;

    let resp = server.post("/dispatch/api/v1/terminals")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "name": "East",
            "timezone": "America/New_York",
            "loaded_rate_per_mile": 0.6
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "expected 201 Created");
    let body = resp.json::<serde_json::Value>();
    assert!(body["id"].as_str().is_some(), "expected id in response body");
    assert_eq!(body["name"], "East");
    assert_eq!(body["timezone"], "America/New_York");
}

// (b) GET list → contains "East" and the seeded "Default"
#[tokio::test]
async fn test_list_terminals_contains_east_and_default() {
    let (server, _b, _d) = test_server().await;
    let token = dispatcher_login(&server, "term2@example.com", "pw-term2").await;
    let auth = format!("Bearer {token}");

    // Create "East"
    server.post("/dispatch/api/v1/terminals")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({
            "name": "East",
            "timezone": "America/New_York",
            "loaded_rate_per_mile": 0.6
        }))
        .await;

    let resp = server.get("/dispatch/api/v1/terminals")
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(resp.status_code(), 200);
    let list: Vec<serde_json::Value> = resp.json();
    let names: Vec<&str> = list.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"East"), "list should contain 'East', got: {names:?}");
    assert!(names.contains(&"Default"), "list should contain seeded 'Default', got: {names:?}");
}

// (c) PUT update loaded_rate_per_mile to 0.7 → 200, value persists
#[tokio::test]
async fn test_update_terminal_rate() {
    let (server, _b, _d) = test_server().await;
    let token = dispatcher_login(&server, "term3@example.com", "pw-term3").await;
    let auth = format!("Bearer {token}");

    let create_resp = server.post("/dispatch/api/v1/terminals")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({
            "name": "East",
            "timezone": "America/New_York",
            "loaded_rate_per_mile": 0.6
        }))
        .await;
    assert_eq!(create_resp.status_code(), 201);
    let id = create_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let update_resp = server.put(&format!("/dispatch/api/v1/terminals/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "loaded_rate_per_mile": 0.7 }))
        .await;
    assert_eq!(update_resp.status_code(), 200, "expected 200 on update");
    let updated = update_resp.json::<serde_json::Value>();
    let rate = updated["loaded_rate_per_mile"].as_f64().unwrap();
    assert!((rate - 0.7).abs() < 1e-9, "expected rate 0.7, got {rate}");
}

// (d) DELETE non-default terminal → 204
#[tokio::test]
async fn test_delete_terminal_returns_204() {
    let (server, _b, _d) = test_server().await;
    let token = dispatcher_login(&server, "term4@example.com", "pw-term4").await;
    let auth = format!("Bearer {token}");

    let create_resp = server.post("/dispatch/api/v1/terminals")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({
            "name": "East",
            "timezone": "America/New_York"
        }))
        .await;
    assert_eq!(create_resp.status_code(), 201);
    let id = create_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let del_resp = server.delete(&format!("/dispatch/api/v1/terminals/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(del_resp.status_code(), 204, "expected 204 No Content on delete");
}

// (e) DELETE the default terminal → 409 Conflict
#[tokio::test]
async fn test_delete_default_terminal_returns_409() {
    let (server, _b, _d) = test_server().await;
    let token = dispatcher_login(&server, "term5@example.com", "pw-term5").await;
    let auth = format!("Bearer {token}");

    // List to find the default terminal
    let list_resp = server.get("/dispatch/api/v1/terminals")
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(list_resp.status_code(), 200);
    let list: Vec<serde_json::Value> = list_resp.json();
    let default_terminal = list.iter().find(|t| t["is_default"].as_bool() == Some(true))
        .expect("a default terminal should be seeded");
    let default_id = default_terminal["id"].as_str().unwrap().to_string();

    let del_resp = server.delete(&format!("/dispatch/api/v1/terminals/{default_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(del_resp.status_code(), 409, "expected 409 Conflict when deleting default terminal");
}

// (e2) PUT {is_default:false} on the sole default → 409 (preserves single-default invariant)
#[tokio::test]
async fn test_unset_default_terminal_returns_409() {
    let (server, _b, _d) = test_server().await;
    let token = dispatcher_login(&server, "term5b@example.com", "pw-term5b").await;
    let auth = format!("Bearer {token}");

    let list_resp = server.get("/dispatch/api/v1/terminals")
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    let list: Vec<serde_json::Value> = list_resp.json();
    let default_id = list.iter().find(|t| t["is_default"].as_bool() == Some(true))
        .expect("a default terminal should be seeded")["id"].as_str().unwrap().to_string();

    let put_resp = server.put(&format!("/dispatch/api/v1/terminals/{default_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "is_default": false }))
        .await;
    assert_eq!(put_resp.status_code(), 409,
        "expected 409 when clearing the default flag on the sole default terminal: {:?}",
        put_resp.text());

    // And the terminal is still the default afterward.
    let still = server.get(&format!("/dispatch/api/v1/terminals/{default_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(still.json::<serde_json::Value>()["is_default"].as_bool(), Some(true));
}

// (f) POST with invalid timezone → 422
#[tokio::test]
async fn test_create_terminal_invalid_timezone_returns_422() {
    let (server, _b, _d) = test_server().await;
    let token = dispatcher_login(&server, "term6@example.com", "pw-term6").await;

    let resp = server.post("/dispatch/api/v1/terminals")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "name": "Bad TZ",
            "timezone": "NotATz"
        }))
        .await;
    assert_eq!(resp.status_code(), 422, "expected 422 for invalid timezone");
}

// (g) PUT {address:null} clears the address (double_option distinguishes absent vs null)
#[tokio::test]
async fn test_patch_terminal_clears_address() {
    let (server, _b, _d) = test_server().await;
    let token = dispatcher_login(&server, "term7@example.com", "pw-term7").await;
    let auth = format!("Bearer {token}");

    let created: serde_json::Value = server.post("/dispatch/api/v1/terminals")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({
            "name": "Addr Yard", "timezone": "America/New_York", "address": "100 Dock St"
        }))
        .await
        .json();
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["address"].as_str(), Some("100 Dock St"));

    // Omitting address must leave it unchanged.
    let unchanged: serde_json::Value = server.put(&format!("/dispatch/api/v1/terminals/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "name": "Addr Yard 2" }))
        .await
        .json();
    assert_eq!(unchanged["address"].as_str(), Some("100 Dock St"), "omitted address should persist");

    // Explicit null clears it.
    let cleared: serde_json::Value = server.put(&format!("/dispatch/api/v1/terminals/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "address": null }))
        .await
        .json();
    assert!(cleared["address"].is_null(), "explicit null should clear address, got: {cleared:?}");
}

// (h) DELETE a terminal with assigned drivers → 409
#[tokio::test]
async fn test_delete_terminal_with_drivers_returns_409() {
    let (server, _b, _d) = test_server().await;
    let token = dispatcher_login(&server, "term8@example.com", "pw-term8").await;
    let auth = format!("Bearer {token}");

    // Create a non-default terminal.
    let term: serde_json::Value = server.post("/dispatch/api/v1/terminals")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "name": "Staffed Yard", "timezone": "America/New_York" }))
        .await
        .json();
    let term_id = term["id"].as_str().unwrap().to_string();

    // Assign a driver to it.
    let drv = server.post("/dispatch/api/v1/drivers")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "name": "Yard Driver", "terminal_id": term_id }))
        .await;
    assert_eq!(drv.status_code(), 201, "driver create failed: {:?}", drv.text());

    // Delete must be refused while a driver is assigned.
    let del = server.delete(&format!("/dispatch/api/v1/terminals/{term_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(del.status_code(), 409,
        "expected 409 deleting a terminal with assigned drivers: {:?}", del.text());
}

/// Returns the id of the seeded Default terminal.
async fn default_terminal_id(server: &TestServer, auth: &str) -> String {
    let resp = server.get("/dispatch/api/v1/terminals")
        .add_header(header::AUTHORIZATION, auth)
        .await;
    let list: Vec<serde_json::Value> = resp.json();
    list.into_iter()
        .find(|t| t["is_default"].as_bool().unwrap_or(false))
        .expect("seeded Default terminal")["id"]
        .as_str().unwrap().to_string()
}

// (g) Driver pay is computed on dispatcher trip detail using terminal-floor rates
//     resolved through trip -> driver -> terminal, then driver override wins.
#[tokio::test]
async fn test_driver_pay_on_trip_detail_uses_resolved_rates() {
    use ollie::models::{TripRecord, TripStatus};

    let (server, db, _b, _d) = test_server_with_db().await;
    let token = dispatcher_login(&server, "pay1@example.com", "pw-pay1").await;
    let auth = format!("Bearer {token}");

    // Set the Default terminal's rate floor.
    let term_id = default_terminal_id(&server, &auth).await;
    let put = server.put(&format!("/dispatch/api/v1/terminals/{term_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({
            "loaded_rate_per_mile": 0.50,
            "deadhead_rate_per_mile": 0.40,
            "extra_stop_fee": 30.0,
            "detention_rate_per_hour": 20.0,
        }))
        .await;
    assert_eq!(put.status_code(), 200);

    // Create a driver on the default terminal (terminal_id defaults to Default).
    let drv = server.post("/dispatch/api/v1/drivers")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "name": "Pat Driver" }))
        .await;
    assert_eq!(drv.status_code(), 201, "driver create failed: {:?}", drv.text());
    let driver_id = uuid::Uuid::parse_str(
        drv.json::<serde_json::Value>()["id"].as_str().unwrap()).unwrap();

    // Insert a trip with miles + stops directly (miles are normally ORS-computed,
    // unavailable in tests). 100 loaded mi, 20 deadhead mi, no detention.
    let now = chrono::Utc::now();
    let trip_id = uuid::Uuid::new_v4();
    let trip = TripRecord {
        id: trip_id,
        trip_number: "T-PAY-0001".into(),
        load_id: None,
        load_number: None,
        previous_trip_id: None,
        deadhead_miles: Some(20.0),
        loaded_miles: Some(100.0),
        total_miles: Some(120.0),
        segment_miles: vec![],
        sequence: 0,
        driver_id: Some(driver_id),
        truck_id: None,
        trailer_ids: vec![],
        status: TripStatus::Planned,
        stops: vec![],
        notes: None,
        blob_ids: vec![],
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
        owner_id: 0,
        created_at: now,
        updated_at: now,
    };
    db.insert_trip(&trip).await.unwrap();

    // GET dispatcher trip detail -> driver_pay computed from terminal floor.
    let detail = server.get(&format!("/dispatch/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(detail.status_code(), 200, "detail GET failed: {:?}", detail.text());
    let body = detail.json::<serde_json::Value>();
    let loaded_miles = body["loaded_miles"].as_f64().unwrap();
    let pay = &body["driver_pay"];
    assert!(!pay.is_null(), "expected driver_pay present, body: {body}");
    let loaded_pay = pay["loaded_pay"].as_f64().unwrap();
    let deadhead_pay = pay["deadhead_pay"].as_f64().unwrap();
    let total_pay = pay["total_pay"].as_f64().unwrap();
    assert!((loaded_pay - loaded_miles * 0.50).abs() < 1e-9,
        "loaded_pay {loaded_pay} != {loaded_miles} * 0.50");
    assert!((deadhead_pay - 20.0 * 0.40).abs() < 1e-9, "deadhead_pay {deadhead_pay}");
    let expected_total = loaded_pay + deadhead_pay
        + pay["extra_stop_pay"].as_f64().unwrap()
        + pay["detention_pay"].as_f64().unwrap();
    assert!((total_pay - expected_total).abs() < 1e-9,
        "total_pay {total_pay} != sum {expected_total}");

    // Driver-level loaded-rate override should win over the terminal floor.
    let patch = server.patch(&format!("/dispatch/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "loaded_rate_per_mile": 0.75 }))
        .await;
    assert_eq!(patch.status_code(), 200, "driver patch failed: {:?}", patch.text());

    let detail2 = server.get(&format!("/dispatch/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    let body2 = detail2.json::<serde_json::Value>();
    let loaded_pay2 = body2["driver_pay"]["loaded_pay"].as_f64().unwrap();
    assert!((loaded_pay2 - loaded_miles * 0.75).abs() < 1e-9,
        "driver override not applied: loaded_pay {loaded_pay2}");
}

// Phase D: settlement freezes driver_pay, locks pay edits + stop times, and
// the pay-period range filter selects the trip.
#[tokio::test]
async fn test_settlement_freezes_pay_and_locks_edits() {
    use ollie::models::{TripRecord, TripStatus, TripStop};
    use ollie::models::trip::TripStopType;

    let (server, db, _b, _d) = test_server_with_db().await;
    let token = dispatcher_login(&server, "settle1@example.com", "pw-settle1").await;
    let auth = format!("Bearer {token}");

    // 1) Set Default terminal rates.
    let term_id = default_terminal_id(&server, &auth).await;
    let put = server.put(&format!("/dispatch/api/v1/terminals/{term_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({
            "loaded_rate_per_mile": 0.50,
            "deadhead_rate_per_mile": 0.40,
            "extra_stop_fee": 30.0,
            "detention_rate_per_hour": 20.0,
        }))
        .await;
    assert_eq!(put.status_code(), 200);

    // Driver on default terminal.
    let drv = server.post("/dispatch/api/v1/drivers")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "name": "Sam Settle" }))
        .await;
    assert_eq!(drv.status_code(), 201, "driver create failed: {:?}", drv.text());
    let driver_id = uuid::Uuid::parse_str(
        drv.json::<serde_json::Value>()["id"].as_str().unwrap()).unwrap();

    // Insert a trip with miles + a stop (so stop_arrive has a target).
    let now = chrono::Utc::now();
    let trip_id = uuid::Uuid::new_v4();
    let stop = TripStop {
        sequence: 0,
        stop_type: TripStopType::Pickup,
        facility_id: None,
        name: Some("Origin Yard".into()),
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
        timezone: Some("America/New_York".into()),
        actual_arrive_utc: None,
        actual_depart_utc: None,
    };
    let trip = TripRecord {
        id: trip_id,
        trip_number: "T-SETTLE-0001".into(),
        load_id: None,
        load_number: None,
        previous_trip_id: None,
        deadhead_miles: Some(20.0),
        loaded_miles: Some(100.0),
        total_miles: Some(120.0),
        segment_miles: vec![],
        sequence: 0,
        driver_id: Some(driver_id),
        truck_id: None,
        trailer_ids: vec![],
        status: TripStatus::InTransit,
        stops: vec![stop],
        notes: None,
        blob_ids: vec![],
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
        owner_id: 0,
        created_at: now,
        updated_at: now,
    };
    db.insert_trip(&trip).await.unwrap();

    // GET detail -> total_pay = T (live).
    let detail = server.get(&format!("/dispatch/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(detail.status_code(), 200, "detail GET failed: {:?}", detail.text());
    let t_total = detail.json::<serde_json::Value>()["driver_pay"]["total_pay"]
        .as_f64().expect("driver_pay present pre-settlement");
    assert!((t_total - (100.0 * 0.50 + 20.0 * 0.40)).abs() < 1e-9, "unexpected T: {t_total}");

    // 2) PATCH settlement_ref + pay periods -> snapshot captured, frozen pay == T.
    let patch = server.patch(&format!("/dispatch/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({
            "settlement_ref": "S-2026-009",
            "pay_period_start": "2026-05-25",
            "pay_period_end": "2026-05-31",
        }))
        .await;
    assert_eq!(patch.status_code(), 200, "settlement patch failed: {:?}", patch.text());

    // Snapshot is persisted on the record; assert via the DB handle.
    let persisted = db.get_trip(trip_id).await.unwrap();
    let snap = persisted.driver_pay_snapshot.expect("snapshot persisted after settlement");
    assert!((snap.total_pay - t_total).abs() < 1e-9, "snapshot total {} != T {t_total}", snap.total_pay);

    let after = server.get(&format!("/dispatch/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    let after_body = after.json::<serde_json::Value>();
    let dp_total = after_body["driver_pay"]["total_pay"].as_f64().unwrap();
    // driver_pay on read must equal the frozen snapshot.
    assert!((dp_total - snap.total_pay).abs() < 1e-9, "driver_pay {dp_total} != snapshot {}", snap.total_pay);

    // 3) Raise the Default terminal loaded rate; settled trip pay stays frozen at T.
    let put2 = server.put(&format!("/dispatch/api/v1/terminals/{term_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "loaded_rate_per_mile": 0.90 }))
        .await;
    assert_eq!(put2.status_code(), 200);
    let after2 = server.get(&format!("/dispatch/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    let frozen_total = after2.json::<serde_json::Value>()["driver_pay"]["total_pay"]
        .as_f64().unwrap();
    assert!((frozen_total - t_total).abs() < 1e-9,
        "settled trip pay must stay frozen: {frozen_total} != {t_total}");

    // 4) PATCH a settled trip's rate override -> 409.
    let locked = server.patch(&format!("/dispatch/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "loaded_rate_per_mile": 0.99 }))
        .await;
    assert_eq!(locked.status_code(), 409, "expected 409 on settled rate edit: {:?}", locked.text());

    // 5) stop_arrive on a settled trip -> 409.
    let arrive = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/stops/0/arrive"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "actual_arrive": "2026-05-28T10:00:00" }))
        .await;
    assert_eq!(arrive.status_code(), 409, "expected 409 on settled stop_arrive: {:?}", arrive.text());

    // 5b) recalculate-miles on a settled trip -> 409 (mileage feeds pay).
    let recalc = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/recalculate-miles"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "force": true }))
        .await;
    assert_eq!(recalc.status_code(), 409, "expected 409 on settled recalculate-miles: {:?}", recalc.text());

    // 5c) PATCH previous_trip_id on a settled trip -> 409 (would trigger a mileage recompute).
    let relink = server.patch(&format!("/dispatch/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "previous_trip_id": uuid::Uuid::new_v4().to_string() }))
        .await;
    assert_eq!(relink.status_code(), 409, "expected 409 on settled previous_trip_id edit: {:?}", relink.text());

    // 5d) Re-settling (changing settlement_ref) on a settled trip -> 409 (no silent drop).
    let resettle = server.patch(&format!("/dispatch/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "settlement_ref": "S-2026-099" }))
        .await;
    assert_eq!(resettle.status_code(), 409, "expected 409 on re-settle: {:?}", resettle.text());
    // And the original ref is untouched.
    assert_eq!(db.get_trip(trip_id).await.unwrap().settlement_ref.as_deref(), Some("S-2026-009"));

    // 5e) An empty settlement_ref is rejected (422), not silently used to freeze.
    let empty_ref = server.patch(&format!("/dispatch/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "settlement_ref": "" }))
        .await;
    assert_eq!(empty_ref.status_code(), 422,
        "expected 422 on empty settlement_ref: {:?}", empty_ref.text());

    // 6) Pay-period range filter includes/excludes the trip.
    let inc = server.get("/dispatch/api/v1/trips?pay_period_start=2026-05-01&pay_period_end=2026-06-30")
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(inc.status_code(), 200);
    let inc_items = inc.json::<serde_json::Value>()["items"].as_array().unwrap().clone();
    assert!(inc_items.iter().any(|t| t["id"].as_str() == Some(&trip_id.to_string())),
        "trip should be in the overlapping pay-period range");

    let exc = server.get("/dispatch/api/v1/trips?pay_period_start=2026-07-01&pay_period_end=2026-07-31")
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(exc.status_code(), 200);
    let exc_items = exc.json::<serde_json::Value>()["items"].as_array().unwrap().clone();
    assert!(!exc_items.iter().any(|t| t["id"].as_str() == Some(&trip_id.to_string())),
        "trip should be excluded from a non-overlapping pay-period range");
}

// ---------------------------------------------------------------------------
// Existing-DB migration guard (#185). Mirrors the `seed_pre_*` pattern in
// tests/migration_test.rs: seed a PRE-SPRINT drivers+trips DB (old Arrow
// schemas WITHOUT this sprint's columns), then open via DbClient::new and
// assert the migration runs without crash-looping (the CAST-null trap), the
// terminals table is created + Default seeded, drivers backfill to the default
// terminal, and the new trip columns round-trip as None/null.
mod migration_guard {
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

    /// Pre-sprint drivers schema: current `driver_schema` MINUS the columns this
    /// sprint added (`terminal_id` + rate overrides + `free_dwell_minutes`).
    fn driver_schema_pre_sprint(embed_dim: usize) -> Arc<Schema> {
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
        ]))
    }

    fn driver_pre_sprint_row(schema: Arc<Schema>, embed_dim: usize) -> RecordBatch {
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
            Arc::new(FixedSizeListArray::from_iter_primitive::<
                arrow_array::types::Float32Type, _, _
            >(nulls, embed_dim as i32)),
            Arc::new(Int64Array::from(vec![1_i64])),
            Arc::new(StringArray::from(vec![Some(now.as_str())])),
            Arc::new(StringArray::from(vec![Some(now.as_str())])),
            Arc::new(StringArray::from(vec![None::<&str>])),  // current_truck_id
            Arc::new(StringArray::from(vec![Some("[]")])),    // current_trailer_ids
            Arc::new(StringArray::from(vec![Some("[]")])),    // blob_ids
        ]).unwrap()
    }

    /// Pre-sprint trips schema: current `trip_schema` MINUS the columns this
    /// sprint added (rate overrides + settlement fields + pay snapshot).
    fn trip_schema_pre_sprint(embed_dim: usize) -> Arc<Schema> {
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
            Field::new("segment_miles", DataType::Utf8, true),
            Field::new("blob_ids", DataType::Utf8, false),
        ]))
    }

    fn trip_pre_sprint_row(schema: Arc<Schema>, embed_dim: usize) -> RecordBatch {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let nulls: Vec<Option<Vec<Option<f32>>>> = vec![None];
        // Two stops so the row resembles a real loaded trip.
        let stops = r#"[
            {"sequence":1,"stop_type":"pickup","facility_id":null},
            {"sequence":2,"stop_type":"delivery","facility_id":null}
        ]"#;
        RecordBatch::try_new(schema, vec![
            Arc::new(StringArray::from(vec![Some(id.as_str())])),
            Arc::new(StringArray::from(vec![Some("T-2026-9001")])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(Int64Array::from(vec![1_i64])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(StringArray::from(vec![Some("[]")])),
            Arc::new(StringArray::from(vec![Some("planned")])),
            Arc::new(StringArray::from(vec![Some(stops)])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(FixedSizeListArray::from_iter_primitive::<
                arrow_array::types::Float32Type, _, _
            >(nulls, embed_dim as i32)),
            Arc::new(Int64Array::from(vec![1_i64])),
            Arc::new(StringArray::from(vec![Some(now.as_str())])),
            Arc::new(StringArray::from(vec![Some(now.as_str())])),
            Arc::new(StringArray::from(vec![None::<&str>])),  // load_number
            Arc::new(StringArray::from(vec![None::<&str>])),  // previous_trip_id
            Arc::new(Float64Array::from(vec![None::<f64>])),  // deadhead_miles
            Arc::new(Float64Array::from(vec![Some(412.5_f64)])), // loaded_miles
            Arc::new(Float64Array::from(vec![None::<f64>])),  // total_miles
            Arc::new(StringArray::from(vec![None::<&str>])),  // segment_miles
            Arc::new(StringArray::from(vec![Some("[]")])),    // blob_ids
        ]).unwrap()
    }

    async fn seed_pre_sprint_db(path: &str) {
        let conn = lancedb::connect(path).execute().await.unwrap();

        let dschema = driver_schema_pre_sprint(EMBED_DIM);
        let dbatch = driver_pre_sprint_row(dschema.clone(), EMBED_DIM);
        let diter = RecordBatchIterator::new(vec![Ok(dbatch)], dschema.clone());
        let dreader: Box<dyn RecordBatchReader + Send> = Box::new(diter);
        conn.create_table("drivers", dreader).execute().await.unwrap();

        let tschema = trip_schema_pre_sprint(EMBED_DIM);
        let tbatch = trip_pre_sprint_row(tschema.clone(), EMBED_DIM);
        let titer = RecordBatchIterator::new(vec![Ok(tbatch)], tschema.clone());
        let treader: Box<dyn RecordBatchReader + Send> = Box::new(titer);
        conn.create_table("trips", treader).execute().await.unwrap();
    }

    #[tokio::test]
    async fn migrates_pre_sprint_db_and_backfills() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();
        seed_pre_sprint_db(path).await;

        // Opening MUST migrate without erroring. If a migration CAST used an
        // Arrow type name instead of a SQL keyword, this crash-loops — this
        // test is the guard for that trap (see AGENTS.md / Critical Constraint #1).
        let client = DbClient::new(path, EMBED_DIM).await.expect(
            "DbClient::new must migrate a pre-sprint drivers+trips DB without erroring. \
             A DataFusion CAST parser error here means a migration used an Arrow type \
             name (e.g. `Int64`) where a SQL keyword (`bigint`) is required.",
        );

        // 1) terminals table created + Default seeded.
        let def = client.default_terminal().await
            .expect("Default terminal must exist after migration");
        assert_eq!(def.name, "Default");

        // 2) pre-existing driver backfilled to the default terminal; overrides None.
        let (_total, drivers) = client.list_drivers(None, 10, 0).await.unwrap();
        assert_eq!(drivers.len(), 1, "the one seeded driver should survive migration");
        let d = client.get_driver_by_id(drivers[0].id).await.unwrap();
        assert_eq!(d.terminal_id, Some(def.id), "driver should backfill to default terminal");
        assert_eq!(d.loaded_rate_per_mile, None);
        assert_eq!(d.deadhead_rate_per_mile, None);
        assert_eq!(d.extra_stop_fee, None);
        assert_eq!(d.detention_rate_per_hour, None);
        assert_eq!(d.free_dwell_minutes, None);

        // 3) pre-existing trip's new columns round-trip as None/null.
        let trips = client.list_trips(None, None, None, None, None).await.unwrap();
        assert_eq!(trips.len(), 1, "the one seeded trip should survive migration");
        let t = &trips[0];
        assert!(t.settlement_ref.is_none());
        assert!(t.driver_pay_snapshot.is_none());
        assert!(t.loaded_rate_per_mile.is_none());
        assert!(t.deadhead_rate_per_mile.is_none());
        assert!(t.extra_stop_fee.is_none());
        assert!(t.detention_rate_per_hour.is_none());
        assert!(t.free_dwell_minutes.is_none());

        // Round-trip via get_trip too (exercises the record-level read path).
        let rec = client.get_trip(t.id).await.unwrap();
        assert!(rec.settlement_ref.is_none());
        assert!(rec.driver_pay_snapshot.is_none());
        assert_eq!(rec.loaded_miles, Some(412.5));
    }
}
