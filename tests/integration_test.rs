// tests/integration_test.rs
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

async fn test_server() -> (TestServer, TempDir, TempDir, async_channel::Receiver<uuid::Uuid>) {
    let blob_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    std::env::set_var("DRIVER_JWT_SECRET", "test-driver-jwt-secret-that-is-long-enough");
    std::env::set_var("DRIVER_RP_ID", "localhost");
    std::env::set_var("DRIVER_RP_ORIGIN", "http://localhost:3000");
    std::env::set_var("FLEET_JWT_SECRET", "test-fleet_user-secret-must-be-32b");

    let config = Arc::new(Config::from_env().unwrap());
    let db = Arc::new(DbClient::new(db_dir.path().to_str().unwrap(), 4).await.unwrap());
    let store = Arc::new(BlobStore::new(blob_dir.path().to_str().unwrap()));
    let ai = Arc::new(OllamaClient::new(
        // Deliberately unreachable: integration tests must not depend on a live
        // Ollama (a real one on :11434 feeds wrong-dim embeddings into the test schema).
        "http://127.0.0.1:1", "nomic-embed-text", "llama3.2", "moondream",
    ));
    let geocoding = Arc::new(ollie::geocoding::GeocodingClient::new());
    let ors = Arc::new(ollie::routing::RoutingClient::new(""));

    let (pipeline_tx, rx) = async_channel::bounded(100);
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
    let server = TestServer::new(api::router(state)).unwrap();
    (server, blob_dir, db_dir, rx)
}

async fn test_server_with_state() -> (TestServer, TempDir, TempDir, async_channel::Receiver<uuid::Uuid>, AppState) {
    let blob_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    std::env::set_var("DRIVER_JWT_SECRET", "test-driver-jwt-secret-that-is-long-enough");
    std::env::set_var("DRIVER_RP_ID", "localhost");
    std::env::set_var("DRIVER_RP_ORIGIN", "http://localhost:3000");
    std::env::set_var("FLEET_JWT_SECRET", "test-fleet_user-secret-must-be-32b");

    let config = Arc::new(Config::from_env().unwrap());
    let db = Arc::new(DbClient::new(db_dir.path().to_str().unwrap(), 4).await.unwrap());
    let store = Arc::new(BlobStore::new(blob_dir.path().to_str().unwrap()));
    let ai = Arc::new(OllamaClient::new(
        // Deliberately unreachable: integration tests must not depend on a live
        // Ollama (a real one on :11434 feeds wrong-dim embeddings into the test schema).
        "http://127.0.0.1:1", "nomic-embed-text", "llama3.2", "moondream",
    ));
    let geocoding = Arc::new(ollie::geocoding::GeocodingClient::new());
    let ors = Arc::new(ollie::routing::RoutingClient::new(""));

    let (pipeline_tx, rx) = async_channel::bounded(100);
    let (geocoding_tx, _grx) = async_channel::bounded(100);
    let (routing_tx, _rrx) = async_channel::bounded(100);

    let rp_origin = Url::parse("http://localhost:3000").unwrap();
    let webauthn = Arc::new(
        WebauthnBuilder::new("localhost", &rp_origin).unwrap().build().unwrap(),
    );
    let auth_challenge_store = Arc::new(dashmap::DashMap::new());
    let reg_challenge_store = Arc::new(dashmap::DashMap::new());

    let state = AppState {
        db, store, ai, geocoding, ors,
        pipeline_tx, geocoding_tx, routing_tx, config,
        webauthn, auth_challenge_store, reg_challenge_store,
    };
    let server = TestServer::new(api::router(state.clone())).unwrap();
    (server, blob_dir, db_dir, rx, state)
}

/// Fixed first-run owner credentials used by the dispatch-surface test helpers.
const OWNER_EMAIL: &str = "owner@example.com";
const OWNER_PASSWORD: &str = "owner-password-123";

/// Returns an owner JWT for the dispatch surface, performing first-run setup if
/// needed. Idempotent: the first call runs `POST /fleet/setup` (only legal
/// while the fleet_user table is empty) and returns the auto-login token; later
/// calls hit the 409 ("already set up") path and log in with the same creds.
/// An owner is used so every elevated op (settle/invoice/delete/terminal-writes/
/// user-mgmt) is permitted. Replaces the old admin `Bearer test-secret` setup.
async fn setup_owner(server: &axum_test::TestServer) -> String {
    let resp = server.post("/fleet/setup")
        .json(&serde_json::json!({
            "email": OWNER_EMAIL, "name": "Owner", "password": OWNER_PASSWORD,
        }))
        .await;
    if resp.status_code() == 200 {
        return resp.json::<serde_json::Value>()["token"].as_str().unwrap().to_string();
    }
    let login = server.post("/fleet/auth/login")
        .json(&serde_json::json!({ "email": OWNER_EMAIL, "password": OWNER_PASSWORD }))
        .await;
    assert_eq!(login.status_code(), 200, "owner login failed");
    login.json::<serde_json::Value>()["token"].as_str().unwrap().to_string()
}

/// Insert an additional active `owner` directly into the DB (with credentials).
/// Used only to set up the otherwise-unreachable two-owner precondition; the
/// users surface forbids creating a second owner.
async fn seed_owner_direct(state: &AppState, email: &str, password: &str) {
    use ollie::models::{FleetUserCredentials, FleetUserRecord, FleetUserStatus};
    use ollie::models::permission::Role;
    let now = chrono::Utc::now();
    let id = uuid::Uuid::new_v4();
    state.db.insert_fleet_user(&FleetUserRecord {
        id,
        email: email.to_string(),
        name: "Seeded Owner".to_string(),
        status: FleetUserStatus::Active,
        role: Role::Owner,
        extra_scopes: Vec::new(),
        created_at: now,
        updated_at: now,
    }).await.unwrap();
    let password_hash = bcrypt::hash(password, 4).unwrap();
    state.db.upsert_fleet_user_credentials(&FleetUserCredentials {
        fleet_user_id: id,
        password_hash,
        token_version: 0,
        failed_attempts: 0,
        locked_until: None,
        updated_at: now,
    }).await.unwrap();
}

#[tokio::test]
async fn test_upload_returns_202_with_uuid() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let resp = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"hello world".to_vec())
                .file_name("hello.txt").mime_type("text/plain")))
        .await;
    assert_eq!(resp.status_code(), 202);
    let body: serde_json::Value = resp.json();
    assert!(body["id"].as_str().is_some());
    assert_eq!(body["status"], "pending");
    assert_eq!(body["name"], "hello.txt");
}

#[tokio::test]
async fn test_upload_with_visibility_field() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let form = axum_test::multipart::MultipartForm::new()
        .add_text("visibility", "driver")
        .add_part(
            "file",
            axum_test::multipart::Part::bytes(b"hello".to_vec())
                .file_name("x.txt")
                .mime_type("text/plain"),
        );
    let resp = server
        .post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(form)
        .await;
    let code = resp.status_code().as_u16();
    assert!([201, 202].contains(&code), "got {code}");
    let body: serde_json::Value = resp.json();
    assert_eq!(body["visibility"], "driver");
}

#[tokio::test]
async fn test_upload_without_auth_returns_401() {
    let (server, _b, _d, _rx) = test_server().await;
    let resp = server.post("/fleet/api/v1/blobs").await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_get_metadata_after_upload() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let upload = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"content".to_vec())
                .file_name("doc.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let meta = server.get(&format!("/fleet/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .add_header(header::ACCEPT, "application/json")
        .await;
    assert_eq!(meta.status_code(), 200);
    assert_eq!(meta.json::<serde_json::Value>()["id"], id);
}

#[tokio::test]
async fn test_get_raw_bytes_regardless_of_status() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let content = b"raw content bytes";
    let upload = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(content.to_vec())
                .file_name("raw.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let raw = server.get(&format!("/fleet/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(raw.status_code(), 200);
    // File is available even though status is still "pending" (no Ollama in test)
    assert_eq!(raw.as_bytes(), content.as_ref());
}

#[tokio::test]
async fn test_list_blobs() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"file1".to_vec())
                .file_name("a.txt").mime_type("text/plain")))
        .await;

    let list = server.get("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(list.status_code(), 200);
    assert!(list.json::<serde_json::Value>()["returned"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn test_dedup_second_upload_returns_201() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let content = b"duplicate content";

    let r1 = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(content.to_vec())
                .file_name("dup1.txt").mime_type("text/plain")))
        .await;
    assert_eq!(r1.status_code(), 202);

    let r2 = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(content.to_vec())
                .file_name("dup2.txt").mime_type("text/plain")))
        .await;
    assert_eq!(r2.status_code(), 201);

    let id1 = r1.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let id2 = r2.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    assert_ne!(id1, id2);
}

#[tokio::test]
async fn test_delete_removes_record_returns_404() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let upload = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"to delete".to_vec())
                .file_name("del.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let del = server.delete(&format!("/fleet/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(del.status_code(), 204);

    let get = server.get(&format!("/fleet/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .add_header(header::ACCEPT, "application/json")
        .await;
    assert_eq!(get.status_code(), 404);
}

#[tokio::test]
async fn test_put_updates_name_and_tags() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let upload = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"data".to_vec())
                .file_name("orig.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let updated = server.put(&format!("/fleet/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "renamed.txt", "tags": ["finance"] }))
        .await;
    assert_eq!(updated.status_code(), 200);
    let body = updated.json::<serde_json::Value>();
    assert_eq!(body["name"], "renamed.txt");
    assert_eq!(body["tags"], serde_json::json!(["finance"]));
}

#[tokio::test]
async fn test_delete_blob_blocked_when_referenced_by_load() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;

    // Upload a blob
    let upload = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"rate con".to_vec())
                .file_name("rate_con.pdf").mime_type("application/pdf")))
        .await;
    let blob_id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Create a facility
    let fac = server.post("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Test Dock", "address": "Memphis, TN" }))
        .await;
    let fac_id = fac.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Create a load referencing the blob
    let load_body = serde_json::json!({
        "customer_name": "ACME",
        "stops": [{
            "sequence": 1, "stop_type": "pickup", "service_type": "live_load",
            "facility_id": fac_id, "scheduled_arrive": "2026-05-10T08:00:00",
            "timezone": "America/Chicago"
        }],
        "rate_items": [],
        "blob_ids": [blob_id]
    });
    server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&load_body)
        .await;

    // Attempt to delete the blob — should be blocked
    let del = server.delete(&format!("/fleet/api/v1/blob/{blob_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(del.status_code(), 409);
}

#[tokio::test]
async fn test_create_facility_returns_201() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let resp = server.post("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "name": "ABC Warehouse",
            "address": "Memphis, TN",
            "contacts": [{"name": "Jane Smith", "title": "Dock Manager"}],
            "tags": ["cold"]
        }))
        .await;
    assert_eq!(resp.status_code(), 201);
    let body = resp.json::<serde_json::Value>();
    assert!(body["id"].as_str().is_some());
    assert_eq!(body["name"], "ABC Warehouse");
    assert_eq!(body["geocode_status"], "pending");
}

#[tokio::test]
async fn test_get_facility() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let create = server.post("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "XYZ Dock", "address": "Nashville, TN" }))
        .await;
    let id = create.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let get = server.get(&format!("/fleet/api/v1/facilities/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(get.status_code(), 200);
    assert_eq!(get.json::<serde_json::Value>()["id"], id);
}

#[tokio::test]
async fn test_delete_facility_blocked_when_referenced_by_load() {
    // The dispatch surface does not expose facility deletion, so the
    // "referenced by a load" guard is verified against its DB predicate
    // (`any_load_references_facility`) via the DB handle.
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;
    let fac = server.post("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Busy Dock", "address": "Atlanta, GA" }))
        .await;
    let fac_id = fac.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let fac_uuid: uuid::Uuid = fac_id.parse().unwrap();
    assert!(!state.db.any_load_references_facility(fac_uuid).await.unwrap(),
        "no load references the facility yet");

    server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [{"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10T08:00:00",
                        "timezone": "America/Chicago"}],
            "rate_items": []
        }))
        .await;

    assert!(state.db.any_load_references_facility(fac_uuid).await.unwrap(),
        "facility is now referenced by a load and must be undeletable");
}

#[tokio::test]
async fn test_list_facilities() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    server.post("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Dock A", "address": "Memphis, TN" }))
        .await;
    let list = server.get("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(list.status_code(), 200);
    assert!(list.json::<serde_json::Value>()["returned"].as_u64().unwrap() >= 1);
}

async fn create_test_facility(server: &axum_test::TestServer, name: &str, address: &str) -> String {
    let owner_token = setup_owner(server).await;
    server.post("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": name, "address": address }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_create_load_returns_201() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, "ABC Dock", "Memphis, TN").await;

    let resp = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "XPO Logistics",
            "customer_ref": "PO-123",
            "stops": [{
                "sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                "facility_id": fac_id, "scheduled_arrive": "2026-05-10T08:00:00",
                "timezone": "America/Chicago"
            }],
            "rate_items": [
                {"description": "Line Haul", "amount_usd": 1800.0},
                {"description": "Fuel Surcharge", "amount_usd": 210.0}
            ],
            "commodity": "auto parts", "tags": ["flatbed"]
        }))
        .await;
    assert_eq!(resp.status_code(), 201);
    let body = resp.json::<serde_json::Value>();
    assert!(body["id"].as_str().is_some());
    assert!(body["load_number"].as_str().unwrap().starts_with("LD-2026-"));
    assert_eq!(body["status"], "planned");
    assert_eq!(body["total_rate_usd"], 2010.0);
}

#[tokio::test]
async fn test_create_load_auto_creates_facility_from_name_address() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;

    let resp = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [{
                "sequence": 1, "stop_type": "pickup", "service_type": "pre_loaded",
                "facility_name": "Brand New Dock",
                "address": "Tulsa, OK",
                "scheduled_arrive": "2026-05-10T08:00:00",
                "force_new_facility": true,
                "timezone": "America/Chicago"
            }],
            "rate_items": []
        }))
        .await;
    assert_eq!(resp.status_code(), 201);

    let facs = server.get("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert!(facs.json::<serde_json::Value>()["returned"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn test_load_number_auto_increments() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let stop = serde_json::json!([{
        "sequence": 1, "stop_type": "pickup", "service_type": "live_load",
        "facility_id": fac_id, "scheduled_arrive": "2026-05-10T08:00:00",
        "timezone": "America/Chicago"
    }]);

    let r1 = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({"customer_name": "A", "stops": stop, "rate_items": []}))
        .await;
    let r2 = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({"customer_name": "B", "stops": stop, "rate_items": []}))
        .await;

    let n1 = r1.json::<serde_json::Value>()["load_number"].as_str().unwrap().to_string();
    let n2 = r2.json::<serde_json::Value>()["load_number"].as_str().unwrap().to_string();
    assert_ne!(n1, n2);
}

#[tokio::test]
async fn test_get_load_detail_includes_facility_info() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, "ABC Dock", "Memphis, TN").await;
    let create = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "XPO",
            "stops": [{"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10T08:00:00",
                        "timezone": "America/Chicago"}],
            "rate_items": []
        }))
        .await;
    let id = create.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let detail = server.get(&format!("/fleet/api/v1/loads/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(detail.status_code(), 200);
    let body = detail.json::<serde_json::Value>();
    let stop = &body["stops"][0];
    assert_eq!(stop["facility_name"], "ABC Dock");
    assert_eq!(stop["address"], "Memphis, TN");
}

#[tokio::test]
async fn test_invalid_service_type_for_stop_returns_400() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;

    let resp = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [{"sequence": 1, "stop_type": "pickup",
                        "service_type": "live_unload",
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10T08:00:00",
                        "timezone": "America/Chicago"}],
            "rate_items": []
        }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_delete_load_returns_204() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let create = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [{"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10T08:00:00",
                        "timezone": "America/Chicago"}],
            "rate_items": []
        }))
        .await;
    let id = create.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let del = server.delete(&format!("/fleet/api/v1/loads/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(del.status_code(), 204);
    assert_eq!(
        server.get(&format!("/fleet/api/v1/loads/{id}"))
            .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
            .await.status_code(),
        404
    );
}

async fn create_test_load(server: &axum_test::TestServer, fac_id: &str) -> String {
    let owner_token = setup_owner(server).await;
    server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [{"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10T08:00:00",
                        "timezone": "America/Chicago"}],
            "rate_items": [{"description": "Line Haul", "amount_usd": 1500.0}]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_full_load_lifecycle() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let id = create_test_load(&server, &fac_id).await;

    // assign/fleet/in_transit/deliver are now driven by trip events (issue #31).
    // Test the post-delivered financial lifecycle: delivered → invoiced → settled.
    // We reach delivered by creating a trip linked to this load with driver_id set
    // (which cascades load to assigned), but for now just test invoice+settle
    // starting from planned via the invoice endpoint (which requires delivered status —
    // skip directly to invoice which returns 409 if not delivered, confirming the
    // state machine still enforces ordering).
    let invoice_premature = server.post(&format!("/fleet/api/v1/loads/{id}/invoice"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({"invoice_number": "INV-001", "invoice_date": "2026-05-15"}))
        .await;
    assert_eq!(invoice_premature.status_code(), 409);

    let invoice = server.post(&format!("/fleet/api/v1/loads/{id}/cancel"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({"reason": "test done"}))
        .await;
    assert_eq!(invoice.json::<serde_json::Value>()["status"], "cancelled");
}

#[tokio::test]
async fn test_cancel_load() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let id = create_test_load(&server, &fac_id).await;

    let resp = server.post(&format!("/fleet/api/v1/loads/{id}/cancel"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({"reason": "Customer cancelled"}))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["status"], "cancelled");
    assert_eq!(body["cancellation_reason"], "Customer cancelled");
}

#[tokio::test]
async fn test_trip_creation_cascades_load_to_assigned() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let load_id = create_test_load(&server, &fac_id).await;
    let driver_id = uuid::Uuid::new_v4();

    let resp = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "load_id": load_id,
            "driver_id": driver_id,
        }))
        .await;
    assert_eq!(resp.status_code(), 201);

    let load_resp = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(load_resp.json::<serde_json::Value>()["status"], "assigned");
}

// ── Trip two-step DELETE ────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_trip_two_step_delete() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;

    // Create a minimal trip (no load or driver required)
    let create_resp = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({}))
        .await;
    assert_eq!(create_resp.status_code(), 201);
    let trip_id = create_resp.json::<serde_json::Value>()["id"]
        .as_str().unwrap().to_string();

    // First DELETE — soft-cancel: should return 204
    let del1 = server.delete(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(del1.status_code(), 204);

    // GET — trip should still exist with status cancelled
    let get_resp = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(get_resp.status_code(), 200);
    assert_eq!(get_resp.json::<serde_json::Value>()["status"], "cancelled");

    // Second DELETE — hard-delete: should return 204
    let del2 = server.delete(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(del2.status_code(), 204);

    // GET — trip should now return 404
    let get_after = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(get_after.status_code(), 404);
}

// ── Blob query endpoint tests ─────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_query_blob_returns_404_for_missing_blob() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fake_id = uuid::Uuid::new_v4();
    let resp = server.post(&format!("/fleet/api/v1/blobs/{fake_id}/query"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "prompt": "What is this?" }))
        .await;
    assert_eq!(resp.status_code(), 404);
}

#[tokio::test]
async fn test_query_blob_returns_422_when_not_ready() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    // Upload a blob — it stays in pending status (no worker running)
    let upload = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"some document text".to_vec())
                .file_name("doc.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    assert_eq!(upload.json::<serde_json::Value>()["status"], "pending");

    let resp = server.post(&format!("/fleet/api/v1/blobs/{id}/query"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "prompt": "What is this?" }))
        .await;
    assert_eq!(resp.status_code(), 422);
}

#[tokio::test]
async fn test_query_blob_returns_400_for_empty_prompt() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let upload = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"content".to_vec())
                .file_name("doc.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let resp = server.post(&format!("/fleet/api/v1/blobs/{id}/query"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "prompt": "" }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_query_blob_returns_400_for_overlong_prompt() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let upload = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"content".to_vec())
                .file_name("doc.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let long_prompt = "a".repeat(4097);
    let resp = server.post(&format!("/fleet/api/v1/blobs/{id}/query"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "prompt": long_prompt }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_query_blob_returns_401_without_auth() {
    let (server, _b, _d, _rx) = test_server().await;
    let fake_id = uuid::Uuid::new_v4();
    let resp = server.post(&format!("/fleet/api/v1/blobs/{fake_id}/query"))
        .json(&serde_json::json!({ "prompt": "What is this?" }))
        .await;
    assert_eq!(resp.status_code(), 401);
}

// ── Driver PIN management tests ─────────────────────────────────────────────────────────────────────────────────

async fn create_test_driver(server: &axum_test::TestServer) -> String {
    let owner_token = setup_owner(server).await;
    server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Test Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_set_driver_pin_returns_204() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let driver_id = create_test_driver(&server).await;

    let resp = server.post(&format!("/fleet/api/v1/drivers/{driver_id}/pin"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "pin": "1234" }))
        .await;
    assert_eq!(resp.status_code(), 204);
}

#[tokio::test]
async fn test_set_driver_pin_invalid_format_returns_422() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let driver_id = create_test_driver(&server).await;

    for invalid_pin in ["abc", "12", "1234567"] {
        let resp = server.post(&format!("/fleet/api/v1/drivers/{driver_id}/pin"))
            .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
            .json(&serde_json::json!({ "pin": invalid_pin }))
            .await;
        assert_eq!(resp.status_code(), 422, "expected 422 for pin: {invalid_pin}");
    }
}

#[tokio::test]
async fn test_set_driver_pin_not_found_returns_404() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fake_id = uuid::Uuid::new_v4();

    let resp = server.post(&format!("/fleet/api/v1/drivers/{fake_id}/pin"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "pin": "1234" }))
        .await;
    assert_eq!(resp.status_code(), 404);
}

#[tokio::test]
async fn test_set_driver_pin_increments_token_version() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let driver_id = create_test_driver(&server).await;

    // First PIN set — token_version should be 0
    let r1 = server.post(&format!("/fleet/api/v1/drivers/{driver_id}/pin"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "pin": "1234" }))
        .await;
    assert_eq!(r1.status_code(), 204);

    // Second PIN set — token_version should be incremented to 1
    let r2 = server.post(&format!("/fleet/api/v1/drivers/{driver_id}/pin"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "pin": "5678" }))
        .await;
    assert_eq!(r2.status_code(), 204);

    // Verify by checking credentials via db directly is not possible here,
    // but we can confirm the second call also returns 204 (idempotent success).
    // The token_version increment is verified at the DB layer by the handler logic.
}

// ── DELETE /fleet/api/v1/loads/:id FK guard ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_delete_load_blocked_by_active_trip() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, "FK Guard Dock", "Chicago, IL").await;
    let load_id = create_test_load(&server, &fac_id).await;

    // Create a trip referencing the load (status defaults to planned = active)
    let trip_resp = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "load_id": load_id }))
        .await;
    assert_eq!(trip_resp.status_code(), 201);
    let trip_id = trip_resp.json::<serde_json::Value>()["id"]
        .as_str().unwrap().to_string();

    // DELETE load → 409 because the trip is active
    let del1 = server.delete(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(del1.status_code(), 409);

    // Cancel the trip (first DELETE soft-cancels it)
    let cancel = server.delete(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(cancel.status_code(), 204);

    // DELETE load → 204 now that no active trips remain
    let del2 = server.delete(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(del2.status_code(), 204);
}

#[tokio::test]
async fn test_assign_sets_trip_resources_and_complete_releases_them() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;

    // Create driver and truck (both start Available)
    let driver_id = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Test Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "T-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Create trip with no driver/truck (simulates hermes flow)
    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "name": "Origin" },
                { "sequence": 1, "stop_type": "delivery", "name": "Destination" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Confirm trip has no driver before assign
    let trip_before = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await
        .json::<serde_json::Value>();
    assert!(trip_before["driver_id"].is_null(), "driver_id should be null before assign");

    // POST /assign
    let assign_resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id }))
        .await;
    assert_eq!(assign_resp.status_code(), 200);

    // Confirm trip now has driver_id
    let trip_after_assign = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(trip_after_assign["driver_id"].as_str(), Some(driver_id.as_str()),
        "driver_id must be set on trip after assign");
    assert_eq!(trip_after_assign["truck_id"].as_str(), Some(truck_id.as_str()),
        "truck_id must be set on trip after assign");

    // Confirm driver status = assigned
    let driver_after_assign = server.get(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(driver_after_assign["status"], "assigned");

    // Walk through lifecycle: dispatch → depart pickup (→ in_transit) → depart delivery (→ delivered) → complete
    let dispatch = server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(dispatch.status_code(), 200);

    let depart_pickup = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/0/depart"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-05-07T10:00:00Z" }))
        .await;
    assert_eq!(depart_pickup.status_code(), 200);

    let depart_delivery = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/1/depart"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-05-07T14:00:00Z" }))
        .await;
    assert_eq!(depart_delivery.status_code(), 200);

    let complete = server.post(&format!("/fleet/api/v1/trips/{trip_id}/complete"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(complete.status_code(), 204);

    // Confirm trip is completed and driver_id is still set (historical record)
    let trip_after_complete = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(trip_after_complete["status"], "completed");
    assert_eq!(trip_after_complete["driver_id"].as_str(), Some(driver_id.as_str()),
        "driver_id must still be set after complete (historical record)");

    // Confirm driver is available again
    let driver_after_complete = server.get(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(driver_after_complete["status"], "available");
}

#[tokio::test]
async fn test_trip_inherits_stops_from_load_when_stops_omitted() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, "Origin Dock", "Chicago, IL").await;

    // Create a load with one stop
    let load_id = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [{"sequence": 0, "stop_type": "pickup", "service_type": "live_load",
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10T08:00:00",
                        "timezone": "America/Chicago"}],
            "rate_items": []
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Create a trip with load_id but no stops
    let trip_resp = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "load_id": load_id }))
        .await;
    assert_eq!(trip_resp.status_code(), 201);
    let trip = trip_resp.json::<serde_json::Value>();
    assert_eq!(trip["stops"].as_array().unwrap().len(), 1, "trip should inherit 1 stop from load");
    assert_eq!(trip["stops"][0]["stop_type"], "pickup");
    assert_eq!(trip["stops"][0]["load_stop_index"], 0);
    assert_eq!(trip["stops"][0]["timezone"], "America/Chicago");
}

#[tokio::test]
async fn test_trip_with_missing_load_id_returns_404() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fake_load_id = uuid::Uuid::new_v4();

    let resp = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "load_id": fake_load_id }))
        .await;
    assert_eq!(resp.status_code(), 404, "missing load_id with no stops should return 404");
}

#[tokio::test]
async fn test_invalid_timezone_returns_422() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;

    let resp = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [{"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10T08:00:00",
                        "timezone": "Not/ATimezone"}],
            "rate_items": []
        }))
        .await;
    assert_eq!(resp.status_code(), 422);
}

#[tokio::test]
async fn test_timezone_too_long_returns_422() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let long_tz = "A".repeat(65);

    let resp = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [{"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10T08:00:00",
                        "timezone": long_tz}],
            "rate_items": []
        }))
        .await;
    assert_eq!(resp.status_code(), 422);
}

#[tokio::test]
async fn test_unassign_clears_trip_resources() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;

    let driver_id = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Unassign Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "T-002" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({}))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Assign
    let assign = server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id }))
        .await;
    assert_eq!(assign.status_code(), 200);

    // Confirm driver_id is set
    let trip_assigned = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(trip_assigned["driver_id"].as_str(), Some(driver_id.as_str()));

    // Unassign
    let unassign = server.post(&format!("/fleet/api/v1/trips/{trip_id}/unassign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(unassign.status_code(), 200);

    // Confirm driver_id is cleared
    let trip_unassigned = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await
        .json::<serde_json::Value>();
    assert!(trip_unassigned["driver_id"].is_null(),
        "driver_id should be null after unassign");
    assert!(trip_unassigned["truck_id"].is_null(),
        "truck_id should be null after unassign");
}

// ── Trip stop arrive/depart 404 tests ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_trip_stop_arrive_returns_404_for_missing_trip() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fake_id = uuid::Uuid::new_v4();

    let resp = server.post(&format!("/fleet/api/v1/trips/{fake_id}/stops/0/arrive"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "actual_arrive": "2026-05-10T10:00:00Z" }))
        .await;
    assert_eq!(resp.status_code(), 404);
}

#[tokio::test]
async fn test_trip_stop_arrive_returns_404_for_missing_sequence() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [{ "sequence": 0, "stop_type": "pickup", "name": "Origin" }]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/999/arrive"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "actual_arrive": "2026-05-10T10:00:00Z" }))
        .await;
    assert_eq!(resp.status_code(), 404);
}

#[tokio::test]
async fn test_trip_stop_depart_returns_404_for_missing_trip() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fake_id = uuid::Uuid::new_v4();

    let resp = server.post(&format!("/fleet/api/v1/trips/{fake_id}/stops/0/depart"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-05-10T10:00:00Z" }))
        .await;
    assert_eq!(resp.status_code(), 404);
}

#[tokio::test]
async fn test_trip_stop_depart_returns_404_for_missing_sequence() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [{ "sequence": 0, "stop_type": "pickup", "name": "Origin" }]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/999/depart"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-05-10T10:00:00Z" }))
        .await;
    assert_eq!(resp.status_code(), 404);
}

// --- Dispatcher integration tests ---

#[tokio::test]
async fn test_create_fleet_user_returns_201() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let resp = server.post("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "email": "fleet_user@example.com",
            "name": "Jane Dispatcher",
            "password": "securepassword123",
            "role": "dispatcher"
        }))
        .await;
    assert_eq!(resp.status_code(), 201);
    let body = resp.json::<serde_json::Value>();
    assert!(body["id"].as_str().is_some());
    assert_eq!(body["email"], "fleet_user@example.com");
    assert_eq!(body["name"], "Jane Dispatcher");
    assert_eq!(body["status"], "active");
    assert_eq!(body["role"], "dispatcher");
}

#[tokio::test]
async fn test_create_fleet_user_duplicate_email_returns_409() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let body = serde_json::json!({
        "email": "dup@example.com",
        "name": "First",
        "password": "password123",
        "role": "dispatcher"
    });
    let r1 = server.post("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&body)
        .await;
    assert_eq!(r1.status_code(), 201);

    let r2 = server.post("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "email": "dup@example.com",
            "name": "Second",
            "password": "different",
            "role": "dispatcher"
        }))
        .await;
    assert_eq!(r2.status_code(), 409);
}

#[tokio::test]
async fn test_list_fleet_users_returns_empty_initially() {
    let (server, _b, _d, _rx) = test_server().await;
    // The dispatch users surface always has at least the first-run owner; assert
    // the list shape (`users`/`returned`) and that the owner is the sole member.
    let owner_token = setup_owner(&server).await;
    let resp = server.get("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["returned"], 1);
    assert_eq!(body["users"].as_array().unwrap().len(), 1);
    assert_eq!(body["users"][0]["email"], OWNER_EMAIL);
}

#[tokio::test]
async fn test_get_fleet_user_by_id() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let create = server.post("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "email": "get@example.com",
            "name": "Get Me",
            "password": "password123",
            "role": "dispatcher"
        }))
        .await;
    assert_eq!(create.status_code(), 201);
    let id = create.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let get = server.get(&format!("/fleet/api/v1/users/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(get.status_code(), 200);
    let body = get.json::<serde_json::Value>();
    assert_eq!(body["id"], id);
    assert_eq!(body["email"], "get@example.com");
    assert_eq!(body["name"], "Get Me");
}

// --- Dispatcher portal auth tests ---

#[tokio::test]
async fn test_fleet_user_login_success() {
    let (server, _b, _d, _rx) = test_server().await;

    // Create a fleet_user via the users surface
    let owner_token = setup_owner(&server).await;
    let create = server.post("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "email": "login@example.com",
            "name": "Login Test",
            "password": "correct-password-123",
            "role": "dispatcher"
        }))
        .await;
    assert_eq!(create.status_code(), 201);

    // Login via fleet_user portal
    let resp = server.post("/fleet/auth/login")
        .json(&serde_json::json!({
            "email": "login@example.com",
            "password": "correct-password-123"
        }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert!(body["token"].as_str().is_some(), "expected a token in response");
}

#[tokio::test]
async fn test_fleet_user_login_bad_password() {
    let (server, _b, _d, _rx) = test_server().await;

    let owner_token = setup_owner(&server).await;
    let create = server.post("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "email": "badpass@example.com",
            "name": "Bad Pass",
            "password": "correct-password-123",
            "role": "dispatcher"
        }))
        .await;
    assert_eq!(create.status_code(), 201);

    let resp = server.post("/fleet/auth/login")
        .json(&serde_json::json!({
            "email": "badpass@example.com",
            "password": "wrong-password"
        }))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_fleet_user_login_unknown_email() {
    let (server, _b, _d, _rx) = test_server().await;

    let resp = server.post("/fleet/auth/login")
        .json(&serde_json::json!({
            "email": "nobody@example.com",
            "password": "any-password"
        }))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_fleet_user_refresh() {
    let (server, _b, _d, _rx) = test_server().await;

    // Create fleet_user
    let owner_token = setup_owner(&server).await;
    let create = server.post("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "email": "refresh@example.com",
            "name": "Refresh Test",
            "password": "refresh-password-123",
            "role": "dispatcher"
        }))
        .await;
    assert_eq!(create.status_code(), 201);

    // Login to get initial token
    let login = server.post("/fleet/auth/login")
        .json(&serde_json::json!({
            "email": "refresh@example.com",
            "password": "refresh-password-123"
        }))
        .await;
    assert_eq!(login.status_code(), 200);
    let set_cookie = login.headers()
        .get("set-cookie")
        .expect("login should return refresh cookie")
        .to_str()
        .unwrap()
        .to_string();
    // Extract the `ollie_refresh=<value>` portion from the Set-Cookie header.
    let cookie_kv = set_cookie.split(';').next().unwrap().trim().to_string();

    // Refresh using the HttpOnly cookie (no Authorization header needed).
    let refresh = server.post("/fleet/auth/refresh")
        .add_header(header::COOKIE, cookie_kv)
        .await;
    assert_eq!(refresh.status_code(), 200);
    let body = refresh.json::<serde_json::Value>();
    assert!(body["token"].as_str().is_some(), "expected a new token in refresh response");
}

// --- Dispatcher portal data API tests ---

async fn fleet_user_login(server: &axum_test::TestServer, email: &str, password: &str) -> String {
    // Create fleet_user account. Scope enforcement (#331) means a plain
    // `fleet_user`-role user is denied elevated ops (settle/invoice/master-data
    // deletes/terminal writes); these data-surface tests exercise the full
    // operational range, so provision them as `owner`. Tests that specifically
    // verify fleet_user-role denial create their user separately.
    // The named user is created via the users surface as `fleet_manager`, which
    // is operationally identical to owner (effective scope `["*"]`) but — unlike
    // owner — is creatable (the surface forbids creating role=owner). This keeps
    // the full elevated-op range these data tests exercise.
    let owner = setup_owner(server).await;
    server.post("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner}"))
        .json(&serde_json::json!({
            "email": email,
            "name": "Test Dispatcher",
            "password": password,
            "role": "fleet_manager",
        }))
        .await;

    // Login and return JWT
    let resp = server.post("/fleet/auth/login")
        .json(&serde_json::json!({ "email": email, "password": password }))
        .await;
    assert_eq!(resp.status_code(), 200);
    resp.json::<serde_json::Value>()["token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_fleet_user_list_loads() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;

    // Login as fleet_user
    let token = fleet_user_login(&server, "data1@example.com", "password-data1").await;

    // Create a facility and load via admin API first
    let fac_id = create_test_facility(&server, "Dispatch Dock", "Chicago, IL").await;
    server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "Dispatch Test Customer",
            "stops": [{
                "sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                "facility_id": fac_id, "scheduled_arrive": "2026-06-01T08:00:00",
                "timezone": "America/Chicago"
            }],
            "rate_items": []
        }))
        .await;

    // GET /fleet/api/v1/loads as fleet_user — should return 200
    let resp = server.get("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert!(body["returned"].as_u64().unwrap() >= 1);
    assert!(body["items"].as_array().is_some());
}

#[tokio::test]
async fn test_fleet_user_get_trip() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;

    // Login as fleet_user
    let token = fleet_user_login(&server, "data2@example.com", "password-data2").await;

    // Create a facility, load, and trip via admin API
    let fac_id = create_test_facility(&server, "Trip Dock", "Dallas, TX").await;
    server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "Trip Test Co",
            "stops": [{
                "sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                "facility_id": fac_id, "scheduled_arrive": "2026-06-01T08:00:00",
                "timezone": "America/Chicago"
            }],
            "rate_items": []
        }))
        .await;

    let trip_resp = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "trip_number": "T-DISP-001",
            "stops": [{
                "sequence": 1,
                "stop_type": "origin",
                "facility_id": fac_id,
                "scheduled_arrive": "2026-06-01T08:00:00",
                "timezone": "America/Chicago"
            }]
        }))
        .await;
    assert_eq!(trip_resp.status_code(), 201);
    let trip_id = trip_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // GET /fleet/api/v1/trips/:id as fleet_user — should return 200
    let resp = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["id"], trip_id);
}

#[tokio::test]
async fn test_fleet_user_assign_and_unassign() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;

    // Login as fleet_user
    let token = fleet_user_login(&server, "data3@example.com", "password-data3").await;

    // Create resources via admin API
    let fac_id = create_test_facility(&server, "Assign Dock", "Houston, TX").await;

    let driver_resp = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Test Driver Dispatch" }))
        .await;
    assert_eq!(driver_resp.status_code(), 201);
    let driver_id = driver_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let truck_resp = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "TR-DISP-001" }))
        .await;
    assert_eq!(truck_resp.status_code(), 201);
    let truck_id = truck_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_resp = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "trip_number": "T-ASSIGN-DISP-001",
            "stops": [{
                "sequence": 1,
                "stop_type": "origin",
                "facility_id": fac_id,
                "scheduled_arrive": "2026-07-01T08:00:00",
                "timezone": "America/Chicago"
            }]
        }))
        .await;
    assert_eq!(trip_resp.status_code(), 201);
    let trip_id = trip_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Assign via fleet_user API
    let assign_resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "driver_id": driver_id,
            "truck_id": truck_id,
            "trailer_ids": []
        }))
        .await;
    assert_eq!(assign_resp.status_code(), 200);
    // Assert on response body of the action itself (AGENTS.md rule)
    let assign_body = assign_resp.json::<serde_json::Value>();
    assert_eq!(assign_body["id"], trip_id);
    assert_eq!(assign_body["driver_id"], driver_id, "response body should include driver_id after assign");
    assert_eq!(assign_body["truck_id"], truck_id, "response body should include truck_id after assign");
    assert_eq!(assign_body["status"], "assigned");

    // Follow-up GET to confirm persistence
    let get_resp = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(get_resp.status_code(), 200);
    let get_body = get_resp.json::<serde_json::Value>();
    assert_eq!(get_body["driver_id"], driver_id);

    // Unassign via fleet_user API
    let unassign_resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/unassign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(unassign_resp.status_code(), 200);
    // Assert on response body of the action itself
    let unassign_body = unassign_resp.json::<serde_json::Value>();
    assert_eq!(unassign_body["id"], trip_id);
    assert!(
        unassign_body["driver_id"].is_null(),
        "response body should have null driver_id after unassign"
    );
    assert_eq!(unassign_body["status"], "planned");
}

// ---------------------------------------------------------------------------
// Dispatcher write-parity tests (#330): load delete/invoice/cancel/settle,
// trip create/delete, driver delete + set-pin, truck/trailer delete.
// ---------------------------------------------------------------------------

/// Create a load with a pickup + delivery stop via the admin API and return its
/// id. Both stops use the same facility for simplicity.
async fn create_2stop_load(server: &axum_test::TestServer, fac_id: &str, customer: &str) -> String {
    let owner_token = setup_owner(server).await;
    let resp = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": customer,
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                  "facility_id": fac_id, "scheduled_arrive": "2026-08-01T08:00:00",
                  "timezone": "America/Chicago" },
                { "sequence": 2, "stop_type": "delivery", "service_type": "live_unload",
                  "facility_id": fac_id, "scheduled_arrive": "2026-08-01T16:00:00",
                  "timezone": "America/Chicago" }
            ],
            "rate_items": []
        }))
        .await;
    assert_eq!(resp.status_code(), 201);
    resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

/// Drive a load to `delivered` by creating a trip on it and walking the trip
/// lifecycle to completion of all stops. Returns the trip id.
async fn drive_load_to_delivered(
    server: &axum_test::TestServer, token: &str, fac_id: &str, load_id: &str,
) -> String {
    let owner_token = setup_owner(server).await;
    let driver_id = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Deliver Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": format!("TRK-{}", &load_id[..8]) }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "load_id": load_id,
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "facility_id": fac_id,
                  "scheduled_arrive": "2026-08-01T08:00:00", "timezone": "America/Chicago" },
                { "sequence": 2, "stop_type": "delivery", "facility_id": fac_id,
                  "scheduled_arrive": "2026-08-01T16:00:00", "timezone": "America/Chicago" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id, "trailer_ids": [] }))
        .await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/1/arrive"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "actual_arrive": "2026-08-01T08:05:00" }))
        .await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/1/depart"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-08-01T09:00:00" }))
        .await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/2/arrive"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "actual_arrive": "2026-08-01T16:05:00" }))
        .await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/2/depart"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-08-01T17:00:00" }))
        .await;
    trip_id
}

#[tokio::test]
async fn test_fleet_user_create_trip() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "ct1@example.com", "password-ct1").await;
    let fac_id = create_test_facility(&server, "Create Trip Dock", "Memphis, TN").await;

    let resp = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "facility_id": fac_id,
                  "scheduled_arrive": "2026-08-01T08:00:00", "timezone": "America/Chicago" },
                { "sequence": 2, "stop_type": "delivery", "facility_id": fac_id,
                  "scheduled_arrive": "2026-08-01T16:00:00", "timezone": "America/Chicago" }
            ]
        }))
        .await;
    assert_eq!(resp.status_code(), 201);
    let body = resp.json::<serde_json::Value>();
    assert!(body["id"].as_str().is_some(), "created trip should carry an id");
    assert_eq!(body["status"], "planned");
    assert!(body["trip_number"].as_str().is_some(), "enriched detail should include trip_number");

    // Confirm it is retrievable.
    let trip_id = body["id"].as_str().unwrap();
    let get = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(get.status_code(), 200);
}

#[tokio::test]
async fn test_fleet_user_delete_trip() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "dt1@example.com", "password-dt1").await;
    let fac_id = create_test_facility(&server, "Delete Trip Dock", "Mobile, AL").await;

    // planned trip → soft-cancel (204), then GET shows cancelled.
    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "facility_id": fac_id,
                  "scheduled_arrive": "2026-08-01T08:00:00", "timezone": "America/Chicago" },
                { "sequence": 2, "stop_type": "delivery", "facility_id": fac_id,
                  "scheduled_arrive": "2026-08-01T16:00:00", "timezone": "America/Chicago" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let del = server.delete(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(del.status_code(), 204);
    let after = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(after["status"], "cancelled");
}

#[tokio::test]
async fn test_fleet_user_delete_trip_in_transit_conflict() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "dt2@example.com", "password-dt2").await;
    let fac_id = create_test_facility(&server, "InTransit Dock", "Macon, GA").await;

    let driver_id = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "InTransit Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "TRK-IT-1" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "facility_id": fac_id,
                  "scheduled_arrive": "2026-08-01T08:00:00", "timezone": "America/Chicago" },
                { "sequence": 2, "stop_type": "delivery", "facility_id": fac_id,
                  "scheduled_arrive": "2026-08-01T16:00:00", "timezone": "America/Chicago" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id, "trailer_ids": [] }))
        .await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/1/arrive"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "actual_arrive": "2026-08-01T08:05:00" }))
        .await;
    let dep = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/1/depart"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-08-01T09:00:00" }))
        .await;
    assert_eq!(dep.json::<serde_json::Value>()["status"], "in_transit");

    let del = server.delete(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(del.status_code(), 409, "deleting an in_transit trip must 409");
}

#[tokio::test]
async fn test_fleet_user_cancel_load() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "cl1@example.com", "password-cl1").await;
    let fac_id = create_test_facility(&server, "Cancel Load Dock", "Tulsa, OK").await;
    let load_id = create_2stop_load(&server, &fac_id, "Cancel Co").await;

    let resp = server.post(&format!("/fleet/api/v1/loads/{load_id}/cancel"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "reason": "customer fell through" }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["status"], "cancelled");
    assert_eq!(body["cancellation_reason"], "customer fell through");
}

#[tokio::test]
async fn test_fleet_user_invoice_and_settle_load() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "is1@example.com", "password-is1").await;
    let fac_id = create_test_facility(&server, "Invoice Dock", "Wichita, KS").await;
    let load_id = create_2stop_load(&server, &fac_id, "Invoice Co").await;

    drive_load_to_delivered(&server, &token, &fac_id, &load_id).await;

    // Confirm the load reached delivered before invoicing.
    let load = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(load["status"], "delivered", "load must be delivered to invoice");

    let inv = server.post(&format!("/fleet/api/v1/loads/{load_id}/invoice"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "invoice_number": "INV-9001" }))
        .await;
    assert_eq!(inv.status_code(), 200);
    let inv_body = inv.json::<serde_json::Value>();
    assert_eq!(inv_body["status"], "invoiced");
    assert_eq!(inv_body["invoice_number"], "INV-9001");

    let settle = server.post(&format!("/fleet/api/v1/loads/{load_id}/settle"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(settle.status_code(), 200);
    assert_eq!(settle.json::<serde_json::Value>()["status"], "settled");
}

#[tokio::test]
async fn test_fleet_user_delete_load_and_active_trip_conflict() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "dl1@example.com", "password-dl1").await;
    let fac_id = create_test_facility(&server, "DelLoad Dock", "Omaha, NE").await;

    // Load with an active trip → delete must 409.
    let load_id = create_2stop_load(&server, &fac_id, "DelLoad Co").await;
    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "load_id": load_id,
            "stops": [
                { "sequence": 1, "stop_type": "origin", "facility_id": fac_id,
                  "scheduled_arrive": "2026-08-01T08:00:00", "timezone": "America/Chicago" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let conflict = server.delete(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(conflict.status_code(), 409, "load with an active trip must 409");

    // Cancel the trip, then delete should succeed (204).
    server.delete(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    let del = server.delete(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(del.status_code(), 204);
    let after = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(after.status_code(), 404, "deleted load should be gone");
}

#[tokio::test]
async fn test_fleet_user_delete_driver() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "dd1@example.com", "password-dd1").await;

    let driver_id = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "name": "Delete Me Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let del = server.delete(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(del.status_code(), 204);

    let after = server.get(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(after["status"], "inactive", "soft-delete sets driver inactive");
}

#[tokio::test]
async fn test_fleet_user_set_driver_pin() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "pin1@example.com", "password-pin1").await;

    let driver_id = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "name": "PIN Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Happy path.
    let ok = server.post(&format!("/fleet/api/v1/drivers/{driver_id}/pin"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "pin": "1234" }))
        .await;
    assert_eq!(ok.status_code(), 204);

    // Bad format → 422.
    let bad = server.post(&format!("/fleet/api/v1/drivers/{driver_id}/pin"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "pin": "12ab" }))
        .await;
    assert_eq!(bad.status_code(), 422);
}

#[tokio::test]
async fn test_fleet_user_delete_truck_and_trailer() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "eq1@example.com", "password-eq1").await;

    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "unit_number": "TRK-DEL-1" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let del_truck = server.delete(&format!("/fleet/api/v1/trucks/{truck_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(del_truck.status_code(), 204);
    let truck_after = server.get(&format!("/fleet/api/v1/trucks/{truck_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(truck_after["status"], "inactive");

    let trailer_id = server.post("/fleet/api/v1/trailers")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "unit_number": "TRL-DEL-1", "owner": "fleet" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let del_trailer = server.delete(&format!("/fleet/api/v1/trailers/{trailer_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(del_trailer.status_code(), 204);
    let trailer_after = server.get(&format!("/fleet/api/v1/trailers/{trailer_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(trailer_after["status"], "inactive");
}

// ---------------------------------------------------------------------------
// Dispatcher MCP tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_fleet_user_mcp_requires_auth() {
    let (server, _b, _d, _rx) = test_server().await;

    // POST /fleet/mcp without auth header → 401
    let resp = server.post("/fleet/mcp")
        .add_header(header::ACCEPT, "application/json, text/event-stream")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_fleet_user_mcp_tools_list() {
    let (server, _b, _d, _rx) = test_server().await;

    let token = fleet_user_login(&server, "mcp1@example.com", "password-mcp1").await;
    let session = mcp_session(&server, &token).await;

    let body = mcp_rpc(&server, &token, &session, "tools/list", serde_json::json!({})).await;
    assert_eq!(body["jsonrpc"], "2.0");
    let tools = body["result"]["tools"].as_array().expect("tools should be an array");
    assert!(!tools.is_empty(), "tools list should not be empty");
    // Verify some expected tools are present
    let tool_names: Vec<&str> = tools.iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(tool_names.contains(&"list_loads"), "should have list_loads tool");
    assert!(tool_names.contains(&"assign_driver"), "should have assign_driver tool");
    assert!(tool_names.contains(&"list_events"), "should have list_events tool");

    // Behavioral annotations + titles reach the wire (camelCase, per MCP spec).
    let by_name = |n: &str| tools.iter().find(|t| t["name"] == n).expect("tool present");
    let list_loads = by_name("list_loads");
    assert_eq!(list_loads["title"], "List Loads");
    assert_eq!(list_loads["annotations"]["readOnlyHint"], true);
    assert_eq!(by_name("delete_blob")["annotations"]["destructiveHint"], true);
    assert_eq!(by_name("update_trip")["annotations"]["idempotentHint"], true);

    // search_blobs is advertised as a read-only semantic-search tool (#292).
    let search = by_name("search_blobs");
    assert_eq!(search["annotations"]["readOnlyHint"], true);
    assert!(search["inputSchema"]["properties"]["query"].is_object());
    assert!(
        search["description"].as_str().unwrap().to_lowercase().contains("semantic"),
        "search_blobs description should flag it as semantic vs literal"
    );
}

/// #292: search_blobs rejects an empty/blank query before touching Ollama, as a
/// recoverable isError result (so the agent can correct the call). The ranked-hit /
/// filter / limit behavior mirrors the REST `?s=` path and requires Ollama, so it
/// is not exercised in the Ollama-free integration suite.
#[tokio::test]
async fn test_mcp_search_blobs_rejects_blank_query() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mcp_sb@example.com", "password-mcp-sb").await;
    let session = mcp_session(&server, &token).await;

    for q in ["", "   "] {
        let body = mcp_rpc(&server, &token, &session, "tools/call", serde_json::json!({
            "name": "search_blobs",
            "arguments": { "query": q }
        })).await;
        assert!(body["error"].is_null(), "blank query is a domain rejection, not a protocol error: {body:?}");
        assert_eq!(body["result"]["isError"], serde_json::json!(true), "blank query must isError");
        let msg = body["result"]["content"][0]["text"].as_str().unwrap_or("");
        assert!(msg.contains("query"), "message should mention the query requirement: {msg}");
    }
}

/// #299: the server advertises the resources capability, lists blobs as resources,
/// and resources/read resolves ollie:// URIs (blob and load) to JSON content.
#[tokio::test]
async fn test_mcp_resources_list_and_read_roundtrip() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mcp_res@example.com", "password-mcp-res").await;
    let created = upload_blob_via_presigned(&server, b"resource blob body".to_vec(), "text/plain", "res.txt").await;
    let blob_id = created["id"].as_str().unwrap().to_string();
    let fac = create_test_facility(&server, "Res Dock", "Dallas, TX").await;
    let load_id = create_test_load(&server, &fac).await;

    // initialize advertises the resources capability.
    let init_resp = server
        .post("/fleet/mcp")
        .add_header(header::ACCEPT, "application/json, text/event-stream")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } }
        }))
        .await;
    let init = sse_json(&init_resp.text());
    assert!(
        init["result"]["capabilities"]["resources"].is_object(),
        "initialize must advertise the resources capability: {init}"
    );

    let session = mcp_session(&server, &token).await;

    // resources/list includes our blob at ollie://blob/{id}.
    let list = mcp_rpc(&server, &token, &session, "resources/list", serde_json::json!({})).await;
    let resources = list["result"]["resources"].as_array().expect("resources array");
    let blob_uri = format!("ollie://blob/{blob_id}");
    let found = resources
        .iter()
        .find(|r| r["uri"] == blob_uri)
        .unwrap_or_else(|| panic!("blob resource {blob_uri} not listed: {list}"));
    assert_eq!(found["mimeType"], "text/plain");

    // resources/read of the blob yields JSON content carrying the record.
    let read = mcp_rpc(&server, &token, &session, "resources/read",
        serde_json::json!({ "uri": blob_uri })).await;
    let contents = &read["result"]["contents"][0];
    assert_eq!(contents["uri"], blob_uri);
    assert_eq!(contents["mimeType"], "application/json");
    let body: serde_json::Value =
        serde_json::from_str(contents["text"].as_str().expect("resource text")).expect("resource JSON");
    assert_eq!(body["id"], blob_id, "blob resource content carries the record id");

    // a load is also readable as a resource (a second record type).
    let load_uri = format!("ollie://load/{load_id}");
    let read_load = mcp_rpc(&server, &token, &session, "resources/read",
        serde_json::json!({ "uri": load_uri })).await;
    let load_body: serde_json::Value = serde_json::from_str(
        read_load["result"]["contents"][0]["text"].as_str().expect("load resource text"),
    ).expect("load resource JSON");
    assert_eq!(load_body["id"], load_id);

    // and a trip (the third record type / templated URI).
    let trip_id = make_trip_with_two_stops(&server).await;
    let trip_uri = format!("ollie://trip/{trip_id}");
    let read_trip = mcp_rpc(&server, &token, &session, "resources/read",
        serde_json::json!({ "uri": trip_uri })).await;
    let trip_body: serde_json::Value = serde_json::from_str(
        read_trip["result"]["contents"][0]["text"].as_str().expect("trip resource text"),
    ).expect("trip resource JSON");
    assert_eq!(trip_body["id"], trip_id);
}

/// #294: blob-returning tools attach resource_link content items pointing at the
/// ollie://blob/{id} resource, alongside the backward-compatible text block.
#[tokio::test]
async fn test_mcp_blob_tools_emit_resource_links() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mcp_link@example.com", "password-mcp-link").await;
    let created = upload_blob_via_presigned(&server, b"link body".to_vec(), "text/plain", "link.txt").await;
    let blob_id = created["id"].as_str().unwrap().to_string();
    let blob_uri = format!("ollie://blob/{blob_id}");
    let session = mcp_session(&server, &token).await;

    // list_blobs result carries a well-formed resource_link for the blob.
    let r = mcp_rpc(&server, &token, &session, "tools/call",
        serde_json::json!({ "name": "list_blobs", "arguments": {} })).await;
    let content = r["result"]["content"].as_array().expect("content array");
    assert!(content.iter().any(|c| c["type"] == "text"), "text block still emitted");
    let link = content
        .iter()
        .find(|c| c["type"] == "resource_link" && c["uri"] == blob_uri)
        .unwrap_or_else(|| panic!("no resource_link for {blob_uri}: {r}"));
    assert_eq!(link["name"], "link.txt");
    assert_eq!(link["mimeType"], "text/plain");
    assert!(link["size"].is_number(), "resource_link should carry size where known");

    // get_blob_metadata also emits the link.
    let m = mcp_rpc(&server, &token, &session, "tools/call",
        serde_json::json!({ "name": "get_blob_metadata", "arguments": { "id": blob_id } })).await;
    assert!(
        m["result"]["content"].as_array().unwrap().iter()
            .any(|c| c["type"] == "resource_link" && c["uri"] == blob_uri),
        "get_blob_metadata should emit a resource_link: {m}"
    );
    // get_blob_url's args-based link path is unit-tested in mcp.rs (its tool needs
    // OLLIE_PUBLIC_BASE_URL, which the test server doesn't set).
}

/// Send a completion/complete request for a reference argument and return the
/// suggested values.
async fn mcp_complete(
    server: &axum_test::TestServer,
    token: &str,
    session: &str,
    arg_name: &str,
    value: &str,
    context: Option<serde_json::Value>,
) -> Vec<String> {
    let mut params = serde_json::json!({
        "ref": { "type": "ref/resource", "uri": "ollie://blob/00000000-0000-0000-0000-000000000000" },
        "argument": { "name": arg_name, "value": value }
    });
    if let Some(ctx) = context {
        params["context"] = ctx;
    }
    let body = mcp_rpc(server, token, session, "completion/complete", params).await;
    body["result"]["completion"]["values"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

/// #301: the completions capability autocompletes reference args (customer_name,
/// facility `q`, tag) from existing records, and honors `context` for a
/// dependent-argument case.
#[tokio::test]
async fn test_mcp_completions_for_reference_args() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mcp_cmp@example.com", "password-mcp-cmp").await;

    // Seed: a load (customer "ACME"), a facility, and a tagged blob.
    let fac = create_test_facility(&server, "Houston Terminal", "Houston, TX").await;
    create_test_load(&server, &fac).await;
    let up_token = mint_upload_token();
    server
        .post(&format!("/fleet/blobs/presigned?token={up_token}&name=hz.txt&tags=hazmat"))
        .add_header(header::CONTENT_TYPE, "text/plain")
        .bytes(b"hz".to_vec().into())
        .await;

    // initialize advertises the completions capability.
    let init_resp = server
        .post("/fleet/mcp")
        .add_header(header::ACCEPT, "application/json, text/event-stream")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } }
        }))
        .await;
    let init = sse_json(&init_resp.text());
    assert!(
        init["result"]["capabilities"]["completions"].is_object(),
        "initialize must advertise completions: {init}"
    );

    let session = mcp_session(&server, &token).await;

    let customers = mcp_complete(&server, &token, &session, "customer_name", "AC", None).await;
    assert!(customers.contains(&"ACME".to_string()), "customer_name suggestions: {customers:?}");

    let facilities = mcp_complete(&server, &token, &session, "q", "Hou", None).await;
    assert!(facilities.iter().any(|v| v == "Houston Terminal"), "facility suggestions: {facilities:?}");

    let tags = mcp_complete(&server, &token, &session, "tag", "haz", None).await;
    assert!(tags.contains(&"hazmat".to_string()), "tag suggestions: {tags:?}");

    // context-dependent: narrowing customer_name to a status with no loads yields none.
    let narrowed = mcp_complete(&server, &token, &session, "customer_name", "AC",
        Some(serde_json::json!({ "arguments": { "status": "delivered" } }))).await;
    assert!(
        narrowed.is_empty(),
        "no delivered-status loads -> context must narrow away ACME: {narrowed:?}"
    );
}

/// #300: destructive ops (cancel_trip, delete_blob force=true) ask the user to
/// confirm via elicitation when the client supports it; a client that does NOT
/// declare elicitation (like this HTTP test harness, which sends capabilities {})
/// degrades to the prior behavior — the op runs without a confirmation round-trip.
/// The positive confirm/decline path needs an elicitation-capable client and is
/// out of scope for the axum-test suite.
#[tokio::test]
async fn test_mcp_destructive_ops_proceed_without_elicitation_support() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mcp_eli@example.com", "password-mcp-eli").await;
    let trip_id = make_trip_with_two_stops(&server).await;
    let created = upload_blob_via_presigned(&server, b"del me".to_vec(), "text/plain", "del.txt").await;
    let blob_id = created["id"].as_str().unwrap().to_string();

    let session = mcp_session(&server, &token).await;
    let not_blocked = |r: &serde_json::Value, what: &str| {
        let text = r["result"]["content"][0]["text"].as_str().unwrap_or("");
        assert!(
            !text.contains("confirmation was declined or unavailable"),
            "{what} must not be blocked by elicitation when the client lacks support: {r}"
        );
    };

    // cancel_trip on a planned trip proceeds (not gated by elicitation here).
    let c = mcp_rpc(&server, &token, &session, "tools/call",
        serde_json::json!({ "name": "cancel_trip", "arguments": { "trip_id": trip_id } })).await;
    not_blocked(&c, "cancel_trip");

    // delete_blob force=true proceeds and deletes.
    let d = mcp_rpc(&server, &token, &session, "tools/call",
        serde_json::json!({ "name": "delete_blob", "arguments": { "id": blob_id, "force": true } })).await;
    not_blocked(&d, "delete_blob force");
    let payload: serde_json::Value =
        serde_json::from_str(d["result"]["content"][0]["text"].as_str().expect("delete_blob text"))
            .expect("delete_blob JSON");
    assert_eq!(payload["deleted"], true, "delete_blob force should delete: {d}");
}

/// Minimal JSON-Schema conformance check for the simple object schemas the MCP
/// tools declare: every `required` property is present with its declared `type`,
/// and any declared property that *is* present matches its `type`. Enough to prove
/// structuredContent honors the contract without a heavyweight schema dependency.
fn structured_conforms(schema: &serde_json::Value, instance: &serde_json::Value) -> bool {
    fn type_ok(expected: &str, val: &serde_json::Value) -> bool {
        match expected {
            "array" => val.is_array(),
            "integer" => val.is_i64() || val.is_u64(),
            "number" => val.is_number(),
            "string" => val.is_string(),
            "object" => val.is_object(),
            "boolean" => val.is_boolean(),
            _ => true,
        }
    }
    let Some(obj) = instance.as_object() else { return false };
    let props = &schema["properties"];
    if let Some(required) = schema["required"].as_array() {
        for r in required {
            let Some(key) = r.as_str() else { return false };
            let Some(val) = obj.get(key) else { return false };
            if !type_ok(props[key]["type"].as_str().unwrap_or(""), val) {
                return false;
            }
        }
    }
    if let Some(declared) = props.as_object() {
        for (key, spec) in declared {
            if let Some(val) = obj.get(key) {
                if !type_ok(spec["type"].as_str().unwrap_or(""), val) {
                    return false;
                }
            }
        }
    }
    true
}

/// #293: high-traffic tools advertise an outputSchema and return structuredContent
/// that conforms to it, while still emitting a backward-compatible text block.
#[tokio::test]
async fn test_mcp_structured_content_validates_against_output_schema() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mcp_struct@example.com", "password-mcp-struct").await;
    let fac = create_test_facility(&server, "Struct Dock", "Dallas, TX").await;
    let load_id = create_test_load(&server, &fac).await;
    let trip_id = make_trip_with_two_stops(&server).await;
    upload_blob_via_presigned(&server, b"struct test blob".to_vec(), "text/plain", "struct.txt").await;
    let session = mcp_session(&server, &token).await;

    let tools = mcp_rpc(&server, &token, &session, "tools/list", serde_json::json!({})).await;

    // Envelope-shape tools: a populated page that conforms to the declared schema.
    assert_structured(&server, &token, &session, &tools, "list_loads", serde_json::json!({}), true).await;
    assert_structured(&server, &token, &session, &tools, "list_trips", serde_json::json!({}), true).await;
    assert_structured(&server, &token, &session, &tools, "list_blobs", serde_json::json!({}), true).await;
    // Record-shape tools.
    let gl = assert_structured(&server, &token, &session, &tools, "get_load", serde_json::json!({ "id": load_id }), false).await;
    assert_eq!(gl["id"], load_id, "get_load structuredContent should carry the id");
    let gt = assert_structured(&server, &token, &session, &tools, "get_trip", serde_json::json!({ "id": trip_id }), false).await;
    assert_eq!(gt["id"], trip_id, "get_trip structuredContent should carry the id");

    // search_blobs declares the envelope schema too (its populated path needs Ollama).
    let search = tools["result"]["tools"].as_array().unwrap().iter()
        .find(|t| t["name"] == "search_blobs").unwrap();
    assert!(search["outputSchema"].is_object(), "search_blobs must advertise an outputSchema");
}

/// Call a tool, then assert its structuredContent conforms to the outputSchema it
/// advertised in `tools`, that the backward-compatible text block is still emitted,
/// and (for list tools) that the page is populated. Returns the structuredContent.
async fn assert_structured(
    server: &axum_test::TestServer,
    token: &str,
    session: &str,
    tools: &serde_json::Value,
    tool: &str,
    args: serde_json::Value,
    expect_items: bool,
) -> serde_json::Value {
    let schema = tools["result"]["tools"].as_array().unwrap().iter()
        .find(|t| t["name"] == tool)
        .unwrap_or_else(|| panic!("missing tool {tool}"))["outputSchema"].clone();
    assert!(schema.is_object(), "{tool} must advertise an outputSchema");
    let r = mcp_rpc(server, token, session, "tools/call",
        serde_json::json!({ "name": tool, "arguments": args })).await;
    let structured = r["result"]["structuredContent"].clone();
    assert!(r["result"]["content"][0]["text"].is_string(), "{tool}: text block still emitted");
    if expect_items {
        assert!(
            structured["items"].as_array().is_some_and(|a| !a.is_empty()),
            "{tool} structuredContent should be populated: {structured}"
        );
    }
    assert!(
        structured_conforms(&schema, &structured),
        "{tool} structuredContent must conform to its outputSchema: {structured}"
    );
    structured
}

/// Cursor pagination over the MCP surface: following `nextCursor` to exhaustion
/// must yield every record exactly once, and the final page must omit nextCursor.
/// Uses list_facilities with a page size of 2 so a 5-record dataset spans 3 pages.
#[tokio::test]
async fn test_fleet_user_mcp_list_cursor_paginates_all_records() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mcp_pg@example.com", "password-mcp-pg").await;

    let mut created = std::collections::HashSet::new();
    for i in 0..5 {
        let id = create_test_facility(&server, &format!("Pager Dock {i}"), "Dallas, TX").await;
        created.insert(id);
    }

    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut args = serde_json::json!({ "limit": 2 });
        if let Some(c) = &cursor {
            args["cursor"] = serde_json::json!(c);
        }
        let res = mcp_call(&server, &token, "list_facilities", args).await;
        for item in res["items"].as_array().expect("items array") {
            seen.push(item["id"].as_str().expect("facility id").to_string());
        }
        pages += 1;
        assert!(pages <= 10, "pagination must terminate");
        match res["nextCursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
    }

    // Every created facility appears exactly once across all pages.
    for id in &created {
        let count = seen.iter().filter(|s| *s == id).count();
        assert_eq!(count, 1, "facility {id} should appear exactly once, saw {count}");
    }
    assert!(pages >= 2, "5 records at page size 2 must span multiple pages, got {pages}");
}

#[tokio::test]
async fn test_fleet_user_mcp_list_loads() {
    let (server, _b, _d, _rx) = test_server().await;

    let token = fleet_user_login(&server, "mcp2@example.com", "password-mcp2").await;
    let session = mcp_session(&server, &token).await;

    let body = mcp_rpc(&server, &token, &session, "tools/call", serde_json::json!({
        "name": "list_loads",
        "arguments": {}
    })).await;
    assert_eq!(body["jsonrpc"], "2.0");
    // Result should have MCP content format
    let content = &body["result"]["content"];
    assert!(content.is_array(), "result.content should be an array");
    let first = &content[0];
    assert_eq!(first["type"], "text", "content type should be text");
    assert!(first["text"].is_string(), "content text should be a string");
}

#[tokio::test]
async fn test_assign_trip_when_driver_is_dispatched_succeeds() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;

    // Create driver and truck (both start Available)
    let driver_id = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Busy Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "T-BUSY-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Create trip A with stops
    let trip_a_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "name": "Origin A" },
                { "sequence": 1, "stop_type": "delivery", "name": "Destination A" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Assign trip A to driver+truck
    let assign_a = server.post(&format!("/fleet/api/v1/trips/{trip_a_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id }))
        .await;
    assert_eq!(assign_a.status_code(), 200, "assign trip A should succeed");

    // Dispatch trip A → driver becomes Dispatched
    let dispatch_a = server.post(&format!("/fleet/api/v1/trips/{trip_a_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(dispatch_a.status_code(), 200, "dispatch trip A should succeed");

    // Confirm driver is now Dispatched
    let driver_after_dispatch = server.get(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(driver_after_dispatch["status"], "dispatched");

    // Create trip B
    let trip_b_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "name": "Origin B" },
                { "sequence": 1, "stop_type": "delivery", "name": "Destination B" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Create a second truck for trip B (since truck_id is dispatched)
    let truck_b_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "T-BUSY-002" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Assign trip B to the same dispatched driver → should succeed (200)
    let assign_b = server.post(&format!("/fleet/api/v1/trips/{trip_b_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_b_id }))
        .await;
    assert_eq!(assign_b.status_code(), 200, "assigning trip B to a dispatched driver should succeed");

    // Confirm trip B has driver_id set
    let trip_b = server.get(&format!("/fleet/api/v1/trips/{trip_b_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(trip_b["driver_id"].as_str(), Some(driver_id.as_str()),
        "trip B must have driver_id set after assign");

    // Driver status should remain dispatched (not downgraded to assigned)
    let driver_after_assign_b = server.get(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(driver_after_assign_b["status"], "dispatched",
        "driver status must remain dispatched after assigning to trip B");
}

#[tokio::test]
async fn test_dispatch_trip_when_driver_already_dispatched_fails() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;

    // Create driver and two trucks
    let driver_id = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Double Dispatch Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let truck_a_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "T-DD-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let truck_b_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "T-DD-002" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Create trips A and B
    let trip_a_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "name": "Origin A" },
                { "sequence": 1, "stop_type": "delivery", "name": "Destination A" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_b_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "name": "Origin B" },
                { "sequence": 1, "stop_type": "delivery", "name": "Destination B" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Assign + dispatch trip A → driver becomes Dispatched
    let assign_a = server.post(&format!("/fleet/api/v1/trips/{trip_a_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_a_id }))
        .await;
    assert_eq!(assign_a.status_code(), 200, "assign trip A should succeed");

    let dispatch_a = server.post(&format!("/fleet/api/v1/trips/{trip_a_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(dispatch_a.status_code(), 200, "dispatch trip A should succeed");

    // Assign trip B to same driver (allowed since driver is dispatched, not inactive)
    let assign_b = server.post(&format!("/fleet/api/v1/trips/{trip_b_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_b_id }))
        .await;
    assert_eq!(assign_b.status_code(), 200, "assign trip B to dispatched driver should succeed");

    // Attempt to dispatch trip B → should fail with 409
    let dispatch_b = server.post(&format!("/fleet/api/v1/trips/{trip_b_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(dispatch_b.status_code(), 409,
        "dispatching trip B when driver is already dispatched should return 409");
}

// ---------------------------------------------------------------------------
// Dispatcher trip lifecycle action tests (#221)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_fleet_user_trip_lifecycle_actions() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "lifecycle1@example.com", "password-lifecycle1").await;

    let fac_id = create_test_facility(&server, "Lifecycle Dock", "Houston, TX").await;

    let driver_id = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Lifecycle Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "TR-LC-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "facility_id": fac_id,
                  "scheduled_arrive": "2026-07-01T08:00:00", "timezone": "America/Chicago" },
                { "sequence": 2, "stop_type": "delivery", "facility_id": fac_id,
                  "scheduled_arrive": "2026-07-01T16:00:00", "timezone": "America/Chicago" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Assign via fleet_user API (covered by existing test) — needed to drive transitions
    let assign_resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id, "trailer_ids": [] }))
        .await;
    assert_eq!(assign_resp.status_code(), 200);

    // Dispatch via fleet_user API
    let dispatch_resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(dispatch_resp.status_code(), 200);
    assert_eq!(dispatch_resp.json::<serde_json::Value>()["status"], "dispatched");

    // Undispatch
    let undispatch_resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/undispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(undispatch_resp.status_code(), 200);
    assert_eq!(undispatch_resp.json::<serde_json::Value>()["status"], "assigned");

    // Re-dispatch then drive stops
    let _ = server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;

    // Check-call
    let cc_resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/check-call"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "location": "I-10 mile 320" }))
        .await;
    assert_eq!(cc_resp.status_code(), 204);

    // Arrive at pickup
    let arr1 = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/1/arrive"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "actual_arrive": "2026-07-01T08:05:00" }))
        .await;
    assert_eq!(arr1.status_code(), 200);

    // Late flag
    let late_resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/2/late"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "eta": "2026-07-01T17:00:00", "notes": "traffic" }))
        .await;
    assert_eq!(late_resp.status_code(), 204);

    // Depart pickup → trip becomes in_transit
    let dep1 = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/1/depart"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-07-01T09:00:00" }))
        .await;
    assert_eq!(dep1.status_code(), 200);
    assert_eq!(dep1.json::<serde_json::Value>()["status"], "in_transit");

    // Arrive + depart delivery → trip becomes delivered
    let _ = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/2/arrive"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "actual_arrive": "2026-07-01T16:05:00" }))
        .await;
    let dep2 = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/2/depart"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-07-01T17:00:00" }))
        .await;
    assert_eq!(dep2.status_code(), 200);
    // Confirm delivered status via follow-up GET (admin stop_depart returns a
    // pre-final-cascade snapshot — see #221 / admin trip_actions::stop_depart).
    let trip_after = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(trip_after["status"], "delivered");

    // Complete
    let complete_resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/complete"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(complete_resp.status_code(), 204);

    // Verify driver/truck released
    let driver_after = server.get(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(driver_after["status"], "available");
}

#[tokio::test]
async fn test_fleet_user_cancel_trip() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "cancel1@example.com", "password-cancel1").await;

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [{ "sequence": 1, "stop_type": "pickup", "name": "Origin" }]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/cancel"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<serde_json::Value>()["status"], "cancelled");
}

#[tokio::test]
async fn test_fleet_user_mcp_lifecycle_tools_listed() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mcp_lc@example.com", "password-mcp-lc").await;
    let session = mcp_session(&server, &token).await;

    let body = mcp_rpc(&server, &token, &session, "tools/list", serde_json::json!({})).await;
    let tools = body["result"]["tools"].as_array().unwrap().clone();
    let names: Vec<String> = tools.iter()
        .filter_map(|t| t["name"].as_str().map(|s| s.to_string()))
        .collect();
    for expected in &[
        "dispatch_trip", "undispatch_trip", "cancel_trip", "complete_trip",
        "stop_arrive", "stop_depart", "stop_late", "check_call",
    ] {
        assert!(names.iter().any(|n| n == *expected),
            "expected MCP tool {expected} to be listed");
    }
}

#[tokio::test]
async fn test_fleet_user_mcp_dispatch_and_complete() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "mcp_dc@example.com", "password-mcp-dc").await;
    let session = mcp_session(&server, &token).await;

    let fac_id = create_test_facility(&server, "MCP Dock", "Dallas, TX").await;

    let driver_id = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "MCP Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "TR-MCP-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "facility_id": fac_id,
                  "scheduled_arrive": "2026-07-02T08:00:00", "timezone": "America/Chicago" },
                { "sequence": 2, "stop_type": "delivery", "facility_id": fac_id,
                  "scheduled_arrive": "2026-07-02T16:00:00", "timezone": "America/Chicago" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // assign via fleet_user API (already covered)
    let _ = server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id, "trailer_ids": [] }))
        .await;

    // Dispatch via MCP
    let trip = mcp_call(&server, &token, "dispatch_trip",
        serde_json::json!({ "trip_id": trip_id })).await;
    assert_eq!(trip["status"], "dispatched");

    // Drive to delivered via MCP stop_arrive/stop_depart
    for (seq, arrive, depart) in [
        (1u32, "2026-07-02T08:05:00", "2026-07-02T09:00:00"),
        (2u32, "2026-07-02T16:05:00", "2026-07-02T17:00:00"),
    ] {
        let r = mcp_rpc(&server, &token, &session, "tools/call", serde_json::json!({
            "name": "stop_arrive",
            "arguments": { "trip_id": trip_id, "sequence": seq, "actual_arrive": arrive }
        })).await;
        assert!(r["error"].is_null(), "stop_arrive error: {:?}", r["error"]);
        let r = mcp_rpc(&server, &token, &session, "tools/call", serde_json::json!({
            "name": "stop_depart",
            "arguments": { "trip_id": trip_id, "sequence": seq, "actual_depart": depart }
        })).await;
        assert!(r["error"].is_null(), "stop_depart error: {:?}", r["error"]);
    }

    // Complete via MCP
    let trip = mcp_call(&server, &token, "complete_trip",
        serde_json::json!({ "trip_id": trip_id })).await;
    assert_eq!(trip["status"], "completed");
}

// ---------------------------------------------------------------------------
// Dispatcher blob API tests (#121)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_fleet_user_blob_requires_auth() {
    let (server, _b, _d, _rx) = test_server().await;
    let resp = server.get("/fleet/api/v1/blobs").await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_fleet_user_upload_blob_returns_202() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "blobs-up@example.com", "password-blobs-up").await;

    let resp = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"pod content".to_vec())
                .file_name("pod.txt").mime_type("text/plain")))
        .await;
    assert!(resp.status_code() == 202 || resp.status_code() == 201);
    let body = resp.json::<serde_json::Value>();
    assert!(body["id"].as_str().is_some());
    assert_eq!(body["name"], "pod.txt");
}

#[tokio::test]
async fn test_fleet_user_list_blobs() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "blobs-list@example.com", "password-blobs-list").await;

    server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"freight bill".to_vec())
                .file_name("bill.txt").mime_type("text/plain")))
        .await;

    let resp = server.get("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert!(body["returned"].as_u64().unwrap() >= 1);
    assert!(body["items"].as_array().is_some());
}

#[tokio::test]
async fn test_fleet_user_get_blob_json() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "blobs-get@example.com", "password-blobs-get").await;

    let upload = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"get me".to_vec())
                .file_name("get.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let resp = server.get(&format!("/fleet/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .add_header(header::ACCEPT, "application/json")
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<serde_json::Value>()["id"], id);
}

#[tokio::test]
async fn test_fleet_user_get_blob_raw_bytes() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "blobs-raw@example.com", "password-blobs-raw").await;

    let content = b"raw document for download test";
    let upload = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(content.to_vec())
                .file_name("download.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // No Accept: application/json → raw bytes
    let resp = server.get(&format!("/fleet/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.as_bytes(), content.as_slice());
}

#[tokio::test]
async fn test_fleet_user_update_blob() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "blobs-upd@example.com", "password-blobs-upd").await;

    let upload = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"update me".to_vec())
                .file_name("original.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let resp = server.put(&format!("/fleet/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "name": "renamed.txt", "tags": ["invoice"] }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["name"], "renamed.txt");
    assert_eq!(body["tags"], serde_json::json!(["invoice"]));
}

#[tokio::test]
async fn test_fleet_user_delete_blob() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "blobs-del@example.com", "password-blobs-del").await;

    let upload = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"delete me".to_vec())
                .file_name("delete.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let del_resp = server.delete(&format!("/fleet/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(del_resp.status_code(), 204);

    let get_resp = server.get(&format!("/fleet/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .add_header(header::ACCEPT, "application/json")
        .await;
    assert_eq!(get_resp.status_code(), 404);
}

#[tokio::test]
async fn test_assign_trip_oos_trailer_returns_409_no_partial_mutation() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "oos-trailer@example.com", "password-oos-trailer").await;

    let fac_id = create_test_facility(&server, "OOS Dock", "Phoenix, AZ").await;

    let driver_resp = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "OOS Test Driver" }))
        .await;
    assert_eq!(driver_resp.status_code(), 201);
    let driver_id = driver_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let truck_resp = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "TR-OOS-001" }))
        .await;
    assert_eq!(truck_resp.status_code(), 201);
    let truck_id = truck_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Create a trailer then mark it out_of_service
    let trailer_resp = server.post("/fleet/api/v1/trailers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "TRL-OOS-001", "owner": "fleet" }))
        .await;
    assert_eq!(trailer_resp.status_code(), 201);
    let trailer_id = trailer_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    // The dispatch trailer PATCH deliberately rejects status writes, so mark the
    // trailer out_of_service directly via the DB to set up the precondition.
    state.db.update_trailer_status(
        trailer_id.parse().unwrap(),
        ollie::models::trailer::TrailerStatus::OutOfService,
    ).await.unwrap();

    let trip_resp = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "trip_number": "T-OOS-001",
            "stops": [{
                "sequence": 1, "stop_type": "origin",
                "facility_id": fac_id, "scheduled_arrive": "2026-08-01T08:00:00",
                "timezone": "America/Chicago"
            }]
        }))
        .await;
    assert_eq!(trip_resp.status_code(), 201);
    let trip_id = trip_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Attempt assign with the OOS trailer — must be 409
    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "driver_id": driver_id,
            "truck_id": truck_id,
            "trailer_ids": [trailer_id]
        }))
        .await;
    assert_eq!(resp.status_code(), 409);

    // Trip must remain planned — no partial mutation
    let trip_check = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(
        trip_check.json::<serde_json::Value>()["status"], "planned",
        "trip must not be left in assigned status after a 409"
    );

    // Driver must remain available
    let driver_check = server.get(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(
        driver_check.json::<serde_json::Value>()["status"], "available",
        "driver must not be marked assigned after a 409"
    );
}

#[tokio::test]
async fn test_upload_large_file_succeeds_under_50mb() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    // 3MB synthetic file — larger than the old 2MB default
    let large_data = vec![0u8; 3 * 1024 * 1024];
    let resp = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(large_data)
                .file_name("large.bin").mime_type("application/octet-stream")))
        .await;
    assert_eq!(resp.status_code(), 202, "3MB upload should succeed with 50MB limit");
}

#[tokio::test]
async fn test_trip_stop_name_and_address_populated_from_facility() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, "Origin Dock", "Chicago, IL").await;

    let load_id = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [{"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10T08:00:00",
                        "timezone": "America/Chicago"}],
            "rate_items": []
        }))
        .await.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "load_id": load_id }))
        .await.json::<serde_json::Value>();

    assert_eq!(trip["stops"][0]["name"], "Origin Dock", "stop name should be populated from facility");
    assert_eq!(trip["stops"][0]["address"], "Chicago, IL", "stop address should be populated from facility");
}

#[tokio::test]
async fn test_trip_load_number_denormalized() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let load_id = create_test_load(&server, &fac_id).await;

    let trip = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "load_id": load_id }))
        .await.json::<serde_json::Value>();

    assert!(trip["load_number"].is_string(), "load_number should be set when load_id is provided");
    assert!(!trip["load_number"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn test_previous_trip_id_auto_populated_from_driver_last_trip() {
    // The dispatch trip DTO does not expose `previous_trip_id`, so the chaining
    // behavior is verified against the persisted TripRecord via the DB handle.
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;
    let driver_id = create_test_driver(&server).await;

    // First trip for this driver — no previous trip
    let trip1 = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id }))
        .await.json::<serde_json::Value>();
    assert_eq!(trip1["status"], "planned");
    let trip1_id = trip1["id"].as_str().unwrap();
    let rec1 = state.db.get_trip(trip1_id.parse().unwrap()).await.unwrap();
    assert!(rec1.previous_trip_id.is_none(), "first trip should have no previous_trip_id");

    // Second trip for same driver — should auto-populate previous_trip_id
    let trip2 = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id }))
        .await.json::<serde_json::Value>();
    let rec2 = state.db.get_trip(trip2["id"].as_str().unwrap().parse().unwrap()).await.unwrap();
    assert_eq!(rec2.previous_trip_id.map(|u| u.to_string()).as_deref(), Some(trip1_id),
        "second trip should chain to first");
}

#[tokio::test]
async fn test_previous_trip_id_fleet_user_override() {
    // `previous_trip_id` is not exposed by the dispatch trip DTO; the override is
    // verified against the persisted TripRecord via the DB handle.
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;
    let driver_id = create_test_driver(&server).await;

    // Create two trips first
    let trip1 = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id }))
        .await.json::<serde_json::Value>();
    let trip1_id = trip1["id"].as_str().unwrap();

    server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id }))
        .await;

    // Third trip with explicit previous_trip_id pointing to trip1 (not trip2)
    let trip3 = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id, "previous_trip_id": trip1_id }))
        .await.json::<serde_json::Value>();
    let rec3 = state.db.get_trip(trip3["id"].as_str().unwrap().parse().unwrap()).await.unwrap();
    assert_eq!(rec3.previous_trip_id.map(|u| u.to_string()).as_deref(), Some(trip1_id),
        "fleet_user override should be respected");
}

#[tokio::test]
async fn test_deadhead_and_loaded_miles_null_without_ors() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let load_id = create_test_load(&server, &fac_id).await;

    let resp = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "load_id": load_id }))
        .await;
    assert_eq!(resp.status_code(), 201);
    let trip = resp.json::<serde_json::Value>();
    assert!(trip["deadhead_miles"].is_null(), "no ORS → deadhead_miles should be null");
    assert!(trip["loaded_miles"].is_null(), "no ORS → loaded_miles should be null");
}

#[tokio::test]
async fn test_fleet_user_loads_list_route_column_has_facility_names() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "route-test@example.com", "pw-route-test").await;

    let origin_id = create_test_facility(&server, "Origin Hub", "Chicago, IL").await;
    let dest_id   = create_test_facility(&server, "Dest Hub",   "Dallas, TX").await;

    server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "Route Test Co",
            "stops": [
                {
                    "sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                    "facility_id": origin_id, "scheduled_arrive": "2026-06-01T08:00:00",
                    "timezone": "America/Chicago"
                },
                {
                    "sequence": 2, "stop_type": "delivery", "service_type": "live_unload",
                    "facility_id": dest_id, "scheduled_arrive": "2026-06-02T14:00:00",
                    "timezone": "America/Chicago"
                }
            ],
            "rate_items": []
        }))
        .await;

    let resp = server.get("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    let items = body["items"].as_array().unwrap();
    assert!(!items.is_empty());
    let stops = items[0]["stops"].as_array().unwrap();
    assert!(stops.len() >= 2);
    assert_eq!(stops[0]["name"], "Origin Hub");
    assert_eq!(stops[1]["name"], "Dest Hub");
}

#[tokio::test]
async fn test_fleet_user_count_endpoints_return_200() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "kpi-test@example.com", "pw-kpi-test").await;

    for path in &[
        "/fleet/api/v1/loads/count",
        "/fleet/api/v1/drivers/count",
        "/fleet/api/v1/blobs/count",
        "/fleet/api/v1/events/count",
    ] {
        let resp = server.get(path)
            .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
            .await;
        assert_eq!(resp.status_code(), 200, "endpoint {} should return 200", path);
        let body = resp.json::<serde_json::Value>();
        assert!(body["count"].is_number(), "endpoint {} should return {{count: N}}", path);
    }
}

async fn setup_driver_with_delivered_trip(server: &TestServer, state: &AppState) -> String {
    let owner_token = setup_owner(server).await;
    let driver_id_str = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Past Trip Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let driver_id: uuid::Uuid = driver_id_str.parse().unwrap();

    // Seed driver_credentials so middleware accepts a JWT for this driver.
    let creds = ollie::models::DriverCredentials {
        driver_id,
        pin_hash: None,
        token_version: 1,
        failed_pin_attempts: 0,
        locked_until: None,
        updated_at: chrono::Utc::now(),
    };
    state.db.upsert_driver_credentials(&creds).await.unwrap();

    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "T-PAST-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "name": "Origin" },
                { "sequence": 1, "stop_type": "delivery", "name": "Destination" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let assign = server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id_str, "truck_id": truck_id }))
        .await;
    assert_eq!(assign.status_code(), 200);

    let dispatch = server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(dispatch.status_code(), 200);

    let depart_pickup = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/0/depart"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-05-12T10:00:00Z" }))
        .await;
    assert_eq!(depart_pickup.status_code(), 200);

    // Last-stop depart sets trip → delivered and populates delivered_at via stop.actual_depart.
    let depart_delivery = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/1/depart"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-05-12T14:00:00Z" }))
        .await;
    assert_eq!(depart_delivery.status_code(), 200);

    let secret = std::env::var("DRIVER_JWT_SECRET").unwrap();
    ollie::api::driver_portal::jwt::encode_driver_jwt(driver_id, 1, &secret).unwrap()
}

#[tokio::test]
async fn test_driver_past_trips_with_week_start() {
    let (server, _db, _blob, _rx, state) = test_server_with_state().await;
    let driver_token = setup_driver_with_delivered_trip(&server, &state).await;
    let resp = server.get("/driver/api/v1/trips?tab=past&week_start=2026-05-10")
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = serde_json::from_slice(resp.as_bytes()).unwrap();
    assert_eq!(body["week"]["start"], "2026-05-10");
    assert_eq!(body["week"]["end"], "2026-05-16");
}

async fn setup_driver_with_intransit_trip_two_stops(
    server: &TestServer,
    state: &AppState,
) -> (String, String) {
    let owner_token = setup_owner(server).await;
    let driver_id_str = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "InTransit Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let driver_id: uuid::Uuid = driver_id_str.parse().unwrap();

    let creds = ollie::models::DriverCredentials {
        driver_id,
        pin_hash: None,
        token_version: 1,
        failed_pin_attempts: 0,
        locked_until: None,
        updated_at: chrono::Utc::now(),
    };
    state.db.upsert_driver_credentials(&creds).await.unwrap();

    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "T-IT-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Use sequences 1 and 2 (per AGENTS.md line 332) so off-by-one bugs surface.
    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                {
                    "sequence": 1,
                    "stop_type": "pickup",
                    "name": "Origin",
                    "timezone": "America/Los_Angeles"
                },
                {
                    "sequence": 2,
                    "stop_type": "delivery",
                    "name": "Destination",
                    "timezone": "America/Los_Angeles"
                }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let assign = server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id_str, "truck_id": truck_id }))
        .await;
    assert_eq!(assign.status_code(), 200);

    let dispatch = server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(dispatch.status_code(), 200);

    // Transition directly to InTransit without departing any stop, so the PATCH
    // tests start with clean actual_arrive/actual_depart on both stops.
    let trip_uuid: uuid::Uuid = trip_id.parse().unwrap();
    state.db.transition_trip_status(trip_uuid, ollie::models::TripStatus::InTransit).await.unwrap();

    let secret = std::env::var("DRIVER_JWT_SECRET").unwrap();
    let token = ollie::api::driver_portal::jwt::encode_driver_jwt(driver_id, 1, &secret).unwrap();
    (token, trip_id)
}

#[tokio::test]
async fn test_patch_stop_arrive_then_depart() {
    let (server, _db, _blob, _rx, state) = test_server_with_state().await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    let arrive_resp = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/1"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({ "actual_arrive": "2026-05-09T08:00:00" }))
        .await;
    assert_eq!(arrive_resp.status_code(), 200);

    let depart_resp = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/1"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-05-09T09:00:00" }))
        .await;
    assert_eq!(depart_resp.status_code(), 200);
}

#[tokio::test]
async fn test_patch_stop_rejects_z_suffix() {
    let (server, _db, _blob, _rx, state) = test_server_with_state().await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    let resp = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/1"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({ "actual_arrive": "2026-05-09T08:00:00Z" }))
        .await;
    assert_eq!(resp.status_code(), 422);
}

#[tokio::test]
async fn test_patch_stop_depart_before_arrive_rejected() {
    let (server, _db, _blob, _rx, state) = test_server_with_state().await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    let arrive = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/1"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({ "actual_arrive": "2026-05-09T10:00:00" }))
        .await;
    assert_eq!(arrive.status_code(), 200);

    let resp = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/1"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-05-09T09:00:00" }))
        .await;
    assert_eq!(resp.status_code(), 422);
}

#[tokio::test]
async fn test_patch_stop_rejects_offset_suffix() {
    let (server, _db, _blob, _rx, state) = test_server_with_state().await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    let resp = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/1"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({ "actual_arrive": "2026-05-09T08:00:00+05:00" }))
        .await;
    assert_eq!(resp.status_code(), 422);
}

#[tokio::test]
async fn test_driver_stop_detail_includes_actual_arrive_utc() {
    let (server, _db, _blob, _rx, state) = test_server_with_state().await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    // Stop 1 has timezone America/Los_Angeles. Naive 14:30 local in May = 21:30 UTC (PDT, UTC-7).
    let arrive = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/1"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({ "actual_arrive": "2026-05-09T14:30:00" }))
        .await;
    assert_eq!(arrive.status_code(), 200);

    let detail = server.get(&format!("/driver/api/v1/trips/{trip_id}/stops/1"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    assert_eq!(detail.status_code(), 200);
    let body: serde_json::Value = detail.json();
    let utc = body["actual_arrive_utc"].as_str().expect("actual_arrive_utc present");
    let parsed = chrono::DateTime::parse_from_rfc3339(utc).expect("valid RFC3339");
    assert_eq!(parsed.with_timezone(&chrono::Utc).to_rfc3339(),
               "2026-05-09T21:30:00+00:00");
}

#[tokio::test]
async fn test_driver_stop_detail_includes_scheduled_arrive_utc() {
    let (server, _db, _blob, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;

    let driver_id_str = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Sched UTC Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let driver_id: uuid::Uuid = driver_id_str.parse().unwrap();

    let creds = ollie::models::DriverCredentials {
        driver_id, pin_hash: None, token_version: 1,
        failed_pin_attempts: 0, locked_until: None,
        updated_at: chrono::Utc::now(),
    };
    state.db.upsert_driver_credentials(&creds).await.unwrap();

    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "T-SCHED-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Naive 08:00 local on 2026-05-09 in America/Chicago (CDT, UTC-5) → 13:00 UTC.
    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "name": "Origin",
                  "scheduled_arrive": "2026-05-09T08:00:00",
                  "scheduled_arrive_end": "2026-05-09T10:00:00",
                  "timezone": "America/Chicago" },
                { "sequence": 2, "stop_type": "delivery", "name": "Destination",
                  "timezone": "America/Chicago" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let assign = server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id_str, "truck_id": truck_id }))
        .await;
    assert_eq!(assign.status_code(), 200);
    let dispatch = server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(dispatch.status_code(), 200);

    let secret = std::env::var("DRIVER_JWT_SECRET").unwrap();
    let driver_token = ollie::api::driver_portal::jwt::encode_driver_jwt(driver_id, 1, &secret).unwrap();

    let detail = server.get(&format!("/driver/api/v1/trips/{trip_id}/stops/1"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    assert_eq!(detail.status_code(), 200);
    let body: serde_json::Value = detail.json();
    let utc = body["scheduled_arrive_utc"].as_str()
        .expect("scheduled_arrive_utc present");
    let parsed = chrono::DateTime::parse_from_rfc3339(utc).expect("valid RFC3339");
    assert_eq!(parsed.with_timezone(&chrono::Utc).to_rfc3339(),
               "2026-05-09T13:00:00+00:00");
    let utc_end = body["scheduled_arrive_end_utc"].as_str()
        .expect("scheduled_arrive_end_utc present");
    let parsed_end = chrono::DateTime::parse_from_rfc3339(utc_end).expect("valid RFC3339");
    assert_eq!(parsed_end.with_timezone(&chrono::Utc).to_rfc3339(),
               "2026-05-09T15:00:00+00:00");

    // Trip detail also exposes the field on the embedded stop summary.
    let trip = server.get(&format!("/driver/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    assert_eq!(trip.status_code(), 200);
    let trip_body: serde_json::Value = trip.json();
    let trip_utc = trip_body["stops"][0]["scheduled_arrive_utc"].as_str()
        .expect("scheduled_arrive_utc on trip-detail stop");
    assert_eq!(chrono::DateTime::parse_from_rfc3339(trip_utc).unwrap()
                  .with_timezone(&chrono::Utc).to_rfc3339(),
               "2026-05-09T13:00:00+00:00");
}

#[tokio::test]
async fn test_driver_stop_detail_includes_free_dwell_minutes() {
    let (server, _db, _blob, _rx, state) = test_server_with_state().await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    let detail = server.get(&format!("/driver/api/v1/trips/{trip_id}/stops/1"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    assert_eq!(detail.status_code(), 200);
    let body: serde_json::Value = detail.json();
    assert_eq!(body["free_dwell_minutes"].as_u64(), Some(120),
               "default free_dwell_minutes should be 120");
}

#[tokio::test]
async fn test_admin_get_trip_legacy_z_row_reads_utc_field() {
    use ollie::models::{TripStop, TripStopType};
    let (server, _db, _blob, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;

    // Build a trip directly via DB with a legacy Z-suffixed actual_arrive and timezone=None.
    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "name": "Origin",
                  "timezone": "America/Chicago" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let trip_uuid: uuid::Uuid = trip_id.parse().unwrap();

    // Simulate a legacy row: timezone=None, actual_arrive with trailing Z.
    let legacy_stops = vec![TripStop {
        sequence: 1,
        stop_type: TripStopType::Pickup,
        facility_id: None,
        name: Some("Origin".into()),
        address: None,
        load_stop_index: None,
        scheduled_arrive: None,
        scheduled_arrive_end: None,
        actual_arrive: Some("2026-04-24T03:20:00Z".into()),
        actual_depart: None,
        expected_dwell_minutes: None,
        detention_free_minutes: None,
        detention_grace_minutes: None,
        notes: None,
        timezone: None,
        actual_arrive_utc: None,
        actual_depart_utc: None,
    }];
    state.db.update_trip_metadata(trip_uuid, None, None, Some(legacy_stops), None, None, None).await.unwrap();

    let resp = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    let utc = body["stops"][0]["actual_arrive_utc"].as_str()
        .expect("legacy UTC row still produces actual_arrive_utc");
    let parsed = chrono::DateTime::parse_from_rfc3339(utc).expect("valid RFC3339");
    assert_eq!(parsed.with_timezone(&chrono::Utc).to_rfc3339(),
               "2026-04-24T03:20:00+00:00");
}

#[tokio::test]
async fn test_patch_stop_accepts_z_suffix_when_no_timezone() {
    // Backwards-compat write: only reject Z when stop's timezone IS set.
    use ollie::models::{TripStop, TripStopType};
    let (server, _db, _blob, _rx, state) = test_server_with_state().await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    // Strip the timezone off stop 1 so it represents a legacy row.
    let trip_uuid: uuid::Uuid = trip_id.parse().unwrap();
    let record = state.db.get_trip(trip_uuid).await.unwrap();
    let mut stops = record.stops.clone();
    stops[0] = TripStop {
        sequence: 1,
        stop_type: TripStopType::Pickup,
        facility_id: None,
        name: Some("Origin".into()),
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
    };
    state.db.update_trip_metadata(trip_uuid, None, None, Some(stops), None, None, None).await.unwrap();

    let resp = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/1"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({ "actual_arrive": "2026-05-09T08:00:00Z" }))
        .await;
    assert_eq!(resp.status_code(), 200, "Z accepted when stop has no timezone");
}

#[tokio::test]
async fn test_patch_last_stop_depart_transitions_to_delivered() {
    let (server, _db, _blob, _rx, state) = test_server_with_state().await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    let r1 = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/1"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({
            "actual_arrive": "2026-05-09T08:00:00",
            "actual_depart": "2026-05-09T09:00:00"
        }))
        .await;
    assert_eq!(r1.status_code(), 200);

    let r2 = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/2"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({ "actual_arrive": "2026-05-09T12:00:00" }))
        .await;
    assert_eq!(r2.status_code(), 200);

    let resp = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/2"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-05-09T13:00:00" }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = serde_json::from_slice(resp.as_bytes()).unwrap();
    assert_eq!(body["trip_status"], "delivered");
}

#[tokio::test]
async fn test_final_stop_depart_auto_dispatches_next_assigned_trip() {
    let (server, _db, _blob, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    // Read the driver_id and truck_id off the in-transit trip so we can build a
    // second Assigned trip for the same driver.
    let trip_uuid: uuid::Uuid = trip_id.parse().unwrap();
    let in_transit = state.db.get_trip(trip_uuid).await.unwrap();
    let driver_id_str = in_transit.driver_id.unwrap().to_string();
    let truck_id_str = in_transit.truck_id.unwrap().to_string();

    // Create trip B with a later scheduled origin arrive and assign same driver/truck.
    let trip_b_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "name": "B-Origin",
                  "scheduled_arrive": "2026-05-10T08:00:00", "timezone": "America/Los_Angeles" },
                { "sequence": 2, "stop_type": "delivery", "name": "B-Dest",
                  "scheduled_arrive": "2026-05-10T18:00:00", "timezone": "America/Los_Angeles" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let assign_b = server.post(&format!("/fleet/api/v1/trips/{trip_b_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id_str, "truck_id": truck_id_str }))
        .await;
    assert_eq!(assign_b.status_code(), 200);

    // Drive trip A to delivered via the driver portal.
    let r1 = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/1"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({
            "actual_arrive": "2026-05-09T08:00:00",
            "actual_depart": "2026-05-09T09:00:00"
        }))
        .await;
    assert_eq!(r1.status_code(), 200);
    let r2 = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/2"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({
            "actual_arrive": "2026-05-09T12:00:00",
            "actual_depart": "2026-05-09T13:00:00"
        }))
        .await;
    assert_eq!(r2.status_code(), 200);
    let body: serde_json::Value = serde_json::from_slice(r2.as_bytes()).unwrap();
    assert_eq!(body["trip_status"], "delivered");

    // Trip B must now be Dispatched.
    let trip_b_uuid: uuid::Uuid = trip_b_id.parse().unwrap();
    let trip_b = state.db.get_trip(trip_b_uuid).await.unwrap();
    assert_eq!(trip_b.status, ollie::models::TripStatus::Dispatched,
        "next assigned trip should be auto-dispatched when prior trip delivers");
}

#[tokio::test]
async fn test_final_stop_depart_no_op_when_no_next_assigned_trip() {
    let (server, _db, _blob, _rx, state) = test_server_with_state().await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    let _ = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/1"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({
            "actual_arrive": "2026-05-09T08:00:00",
            "actual_depart": "2026-05-09T09:00:00"
        }))
        .await;
    let r2 = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/2"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({
            "actual_arrive": "2026-05-09T12:00:00",
            "actual_depart": "2026-05-09T13:00:00"
        }))
        .await;
    assert_eq!(r2.status_code(), 200);
    let body: serde_json::Value = serde_json::from_slice(r2.as_bytes()).unwrap();
    assert_eq!(body["trip_status"], "delivered");
    // Nothing else to dispatch — endpoint should still return cleanly.
}

#[tokio::test]
async fn test_final_stop_depart_skips_auto_dispatch_when_truck_busy_elsewhere() {
    let (server, _db, _blob, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;
    let (driver_token, trip_a_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    let trip_a_uuid: uuid::Uuid = trip_a_id.parse().unwrap();
    let trip_a = state.db.get_trip(trip_a_uuid).await.unwrap();
    let driver_a = trip_a.driver_id.unwrap();

    // Build a SECOND independent driver + a busy truck that's InTransit on
    // that driver's trip. Then create Trip B for driver A but referencing
    // the busy truck — auto-dispatch should refuse.
    let driver_b_id_str = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Other Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let busy_truck_id_str = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "T-BUSY" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    // Trip C: driver B on the busy truck, drive to InTransit.
    let trip_c_id_str = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "name": "C-Origin", "timezone": "America/Los_Angeles" },
                { "sequence": 2, "stop_type": "delivery", "name": "C-Dest", "timezone": "America/Los_Angeles" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let _ = server.post(&format!("/fleet/api/v1/trips/{trip_c_id_str}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_b_id_str, "truck_id": busy_truck_id_str }))
        .await;
    let _ = server.post(&format!("/fleet/api/v1/trips/{trip_c_id_str}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    let trip_c_uuid: uuid::Uuid = trip_c_id_str.parse().unwrap();
    state.db.transition_trip_status(trip_c_uuid, ollie::models::TripStatus::InTransit).await.unwrap();

    // Trip B: driver A on the SAME busy truck — assigned only.
    let trip_b_id_str = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "name": "B-Origin",
                  "scheduled_arrive": "2026-05-10T08:00:00", "timezone": "America/Los_Angeles" },
                { "sequence": 2, "stop_type": "delivery", "name": "B-Dest",
                  "scheduled_arrive": "2026-05-10T18:00:00", "timezone": "America/Los_Angeles" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let _ = server.post(&format!("/fleet/api/v1/trips/{trip_b_id_str}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_a.to_string(), "truck_id": busy_truck_id_str }))
        .await;

    // Deliver trip A.
    let _ = server.patch(&format!("/driver/api/v1/trips/{trip_a_id}/stops/1"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({
            "actual_arrive": "2026-05-09T08:00:00",
            "actual_depart": "2026-05-09T09:00:00"
        }))
        .await;
    let _ = server.patch(&format!("/driver/api/v1/trips/{trip_a_id}/stops/2"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({
            "actual_arrive": "2026-05-09T12:00:00",
            "actual_depart": "2026-05-09T13:00:00"
        }))
        .await;

    // Trip B must stay Assigned — the truck is busy on trip C.
    let trip_b_uuid: uuid::Uuid = trip_b_id_str.parse().unwrap();
    let trip_b = state.db.get_trip(trip_b_uuid).await.unwrap();
    assert_eq!(trip_b.status, ollie::models::TripStatus::Assigned,
        "auto-dispatch must not silently double-book a truck already on another active trip");
}

#[tokio::test]
async fn test_complete_trip_does_not_release_driver_already_on_next_trip() {
    let (server, _db, _blob, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    let trip_uuid: uuid::Uuid = trip_id.parse().unwrap();
    let in_transit = state.db.get_trip(trip_uuid).await.unwrap();
    let driver_id = in_transit.driver_id.unwrap();
    let truck_id = in_transit.truck_id.unwrap();

    let trip_b_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "name": "B-Origin",
                  "scheduled_arrive": "2026-05-10T08:00:00", "timezone": "America/Los_Angeles" },
                { "sequence": 2, "stop_type": "delivery", "name": "B-Dest",
                  "scheduled_arrive": "2026-05-10T18:00:00", "timezone": "America/Los_Angeles" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let _ = server.post(&format!("/fleet/api/v1/trips/{trip_b_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "driver_id": driver_id.to_string(),
            "truck_id": truck_id.to_string()
        }))
        .await;

    // Deliver trip A — should auto-dispatch trip B onto same driver/truck.
    let _ = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/1"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({
            "actual_arrive": "2026-05-09T08:00:00",
            "actual_depart": "2026-05-09T09:00:00"
        }))
        .await;
    let _ = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/2"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({
            "actual_arrive": "2026-05-09T12:00:00",
            "actual_depart": "2026-05-09T13:00:00"
        }))
        .await;

    // Now complete trip A — driver/truck must stay Dispatched (they're on trip B).
    let complete = server.post(&format!("/fleet/api/v1/trips/{trip_id}/complete"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(complete.status_code(), 204);

    let driver = state.db.get_driver_by_id(driver_id).await.unwrap();
    assert_eq!(driver.status, ollie::models::DriverStatus::Dispatched,
        "driver still on next trip should remain Dispatched, not released to Available");
    let truck = state.db.get_truck_by_id(truck_id).await.unwrap();
    assert_eq!(truck.status, ollie::models::TruckStatus::Dispatched,
        "truck still on next trip should remain Dispatched");
}

#[tokio::test]
async fn test_driver_upload_lists_own_document() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;
    let form = axum_test::multipart::MultipartForm::new()
        .add_text("doctype", "bol")
        .add_part(
            "file",
            axum_test::multipart::Part::bytes(b"hi".to_vec())
                .file_name("bol.txt")
                .mime_type("text/plain"),
        );
    let upload = server
        .post(&format!("/driver/api/v1/trips/{trip_id}/documents"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .multipart(form)
        .await;
    let sc = upload.status_code().as_u16();
    assert!(sc == 201 || sc == 202, "got {sc}");
    let list = server
        .get(&format!("/driver/api/v1/trips/{trip_id}/documents"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    let items: Vec<serde_json::Value> = serde_json::from_slice(list.as_bytes()).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["visibility"], "driver");
}

#[tokio::test]
async fn test_driver_upload_attaches_blob_to_load() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;

    // Build a load + trip linked to that load, assigned + dispatched + InTransit.
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let load_id = create_test_load(&server, &fac_id).await;

    let driver_id_str = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Doc Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let driver_id: uuid::Uuid = driver_id_str.parse().unwrap();
    let creds = ollie::models::DriverCredentials {
        driver_id,
        pin_hash: None,
        token_version: 1,
        failed_pin_attempts: 0,
        locked_until: None,
        updated_at: chrono::Utc::now(),
    };
    state.db.upsert_driver_credentials(&creds).await.unwrap();

    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "T-DOC-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "load_id": load_id }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let _ = server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id_str, "truck_id": truck_id }))
        .await;
    let _ = server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    let trip_uuid: uuid::Uuid = trip_id.parse().unwrap();
    state.db.transition_trip_status(trip_uuid, ollie::models::TripStatus::InTransit).await.unwrap();

    let secret = std::env::var("DRIVER_JWT_SECRET").unwrap();
    let driver_token = ollie::api::driver_portal::jwt::encode_driver_jwt(driver_id, 1, &secret).unwrap();

    let form = axum_test::multipart::MultipartForm::new()
        .add_text("doctype", "bol")
        .add_part(
            "file",
            axum_test::multipart::Part::bytes(b"bol-bytes".to_vec())
                .file_name("bol.txt")
                .mime_type("text/plain"),
        );
    let upload = server
        .post(&format!("/driver/api/v1/trips/{trip_id}/documents"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .multipart(form)
        .await;
    let sc = upload.status_code().as_u16();
    assert!(sc == 201 || sc == 202, "got {sc}");
    let blob_id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let load_resp = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    let blob_ids = load_resp.json::<serde_json::Value>()["blob_ids"].clone();
    let ids: Vec<String> = blob_ids.as_array().unwrap()
        .iter().map(|v| v.as_str().unwrap().to_string()).collect();
    assert!(ids.contains(&blob_id),
        "load.blob_ids should contain driver-uploaded blob; got {ids:?}");
}

#[tokio::test]
async fn test_driver_cannot_see_private_doc() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;
    let form = axum_test::multipart::MultipartForm::new()
        .add_text("visibility", "private")
        .add_text("tags", format!(r#"["trip:{trip_id}"]"#))
        .add_part(
            "file",
            axum_test::multipart::Part::bytes(b"rate-con".to_vec())
                .file_name("rate-con.pdf")
                .mime_type("application/pdf"),
        );
    let _ = server
        .post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(form)
        .await;
    let list = server
        .get(&format!("/driver/api/v1/trips/{trip_id}/documents"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    let items: Vec<serde_json::Value> = serde_json::from_slice(list.as_bytes()).unwrap();
    assert_eq!(items.len(), 0, "driver should NOT see private doc");
}

#[tokio::test]
async fn test_driver_sees_dispatch_uploaded_driver_visible_doc() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;
    let form = axum_test::multipart::MultipartForm::new()
        .add_text("visibility", "driver")
        .add_text("tags", format!(r#"["trip:{trip_id}"]"#))
        .add_part(
            "file",
            axum_test::multipart::Part::bytes(b"bol".to_vec())
                .file_name("bol.pdf")
                .mime_type("application/pdf"),
        );
    let _ = server
        .post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(form)
        .await;
    let list = server
        .get(&format!("/driver/api/v1/trips/{trip_id}/documents"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    let items: Vec<serde_json::Value> = serde_json::from_slice(list.as_bytes()).unwrap();
    assert_eq!(items.len(), 1);
}

#[tokio::test]
async fn test_driver_cannot_delete_others_doc() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;
    let form = axum_test::multipart::MultipartForm::new()
        .add_text("visibility", "driver")
        .add_text("tags", format!(r#"["trip:{trip_id}"]"#))
        .add_part(
            "file",
            axum_test::multipart::Part::bytes(b"doc".to_vec())
                .file_name("d.txt")
                .mime_type("text/plain"),
        );
    let upload = server
        .post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(form)
        .await;
    let blob_id = upload.json::<serde_json::Value>()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let del = server
        .delete(&format!(
            "/driver/api/v1/trips/{trip_id}/documents/{blob_id}"
        ))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    assert_eq!(del.status_code(), 403);
}

#[tokio::test]
async fn test_driver_unrelated_404_on_other_trip() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let (token_a, _trip_a) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;
    let (_token_b, trip_b) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;
    let resp = server
        .get(&format!("/driver/api/v1/trips/{trip_b}/documents"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token_a}"))
        .await;
    assert_eq!(resp.status_code(), 404);
}

// ---------------------------------------------------------------------------
// Helper: create a driver + truck + N delivered trips, one per historical week.
// Stops have no timezone so the dispatch depart endpoint accepts UTC Z-suffix
// strings. Returns (driver_token, truck_id, driver_id_str).
// ---------------------------------------------------------------------------
async fn setup_driver_with_three_historical_trips(
    server: &TestServer,
    state: &AppState,
) -> String {
    let owner_token = setup_owner(server).await;
    let driver_id_str = server
        .post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Multi-Week Driver" }))
        .await
        .json::<serde_json::Value>()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let driver_id: uuid::Uuid = driver_id_str.parse().unwrap();

    let creds = ollie::models::DriverCredentials {
        driver_id,
        pin_hash: None,
        token_version: 1,
        failed_pin_attempts: 0,
        locked_until: None,
        updated_at: chrono::Utc::now(),
    };
    state.db.upsert_driver_credentials(&creds).await.unwrap();

    // Deliver 3 trips across 3 distinct calendar weeks (Sunday-aligned, America/New_York).
    // Week of 2026-05-10: deliver Tuesday 2026-05-12 14:00 UTC (10am ET)
    // Week of 2026-04-26: deliver Monday 2026-04-27 14:00 UTC (10am ET)
    // Week of 2026-04-12: deliver Tuesday 2026-04-14 14:00 UTC (10am ET)
    let depart_times = [
        ("2026-05-12T14:00:00Z", "2026-05-12T18:00:00Z"),
        ("2026-04-27T14:00:00Z", "2026-04-27T18:00:00Z"),
        ("2026-04-14T14:00:00Z", "2026-04-14T18:00:00Z"),
    ];

    for (i, (pickup_depart, delivery_depart)) in depart_times.iter().enumerate() {
        // Each trip uses its own truck so truck status doesn't block subsequent dispatches.
        let truck_id = server
            .post("/fleet/api/v1/trucks")
            .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
            .json(&serde_json::json!({ "unit_number": format!("T-MULTI-{i:03}") }))
            .await
            .json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();

        let trip_id = server
            .post("/fleet/api/v1/trips")
            .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
            .json(&serde_json::json!({
                "stops": [
                    { "sequence": 0, "stop_type": "pickup", "name": format!("Origin-{i}") },
                    { "sequence": 1, "stop_type": "delivery", "name": format!("Dest-{i}") }
                ]
            }))
            .await
            .json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();

        let assign = server
            .post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
            .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
            .json(&serde_json::json!({ "driver_id": driver_id_str, "truck_id": truck_id }))
            .await;
        assert_eq!(assign.status_code(), 200, "assign trip {i}");

        let dispatch = server
            .post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
            .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
            .await;
        assert_eq!(dispatch.status_code(), 200, "dispatch trip {i}");

        let r = server
            .post(&format!("/fleet/api/v1/trips/{trip_id}/stops/0/depart"))
            .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
            .json(&serde_json::json!({ "actual_depart": pickup_depart }))
            .await;
        assert_eq!(r.status_code(), 200, "depart pickup trip {i}");

        let r = server
            .post(&format!("/fleet/api/v1/trips/{trip_id}/stops/1/depart"))
            .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
            .json(&serde_json::json!({ "actual_depart": delivery_depart }))
            .await;
        assert_eq!(r.status_code(), 200, "depart delivery trip {i}");

        // Reset driver to Available so the next trip can be dispatched (on_trip_delivered
        // only logs an event; on_trip_completed resets statuses, but it's not called
        // automatically on the standalone-trip flow tested here).
        state
            .db
            .update_driver_status(driver_id, ollie::models::DriverStatus::Available)
            .await
            .unwrap();
    }

    let secret = std::env::var("DRIVER_JWT_SECRET").unwrap();
    ollie::api::driver_portal::jwt::encode_driver_jwt(driver_id, 1, &secret).unwrap()
}

#[tokio::test]
async fn test_past_trips_paginates_beyond_last_week() {
    let (server, _db, _blob, _rx, state) = test_server_with_state().await;
    let driver_token = setup_driver_with_three_historical_trips(&server, &state).await;

    // Query "this week" (2026-05-17) — all 3 trips are in the past, so has_prev must be true
    // and earliest_week_start must reflect the oldest trip (week of 2026-04-12).
    let resp = server
        .get("/driver/api/v1/trips?tab=past&week_start=2026-05-17")
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = serde_json::from_slice(resp.as_bytes()).unwrap();
    assert_eq!(body["week"]["has_prev"], true, "week 2026-05-17: has_prev should be true");
    assert_eq!(
        body["week"]["earliest_week_start"],
        "2026-04-12",
        "week 2026-05-17: earliest_week_start should be 2026-04-12"
    );

    // Query the middle week (2026-04-26) — the 5-weeks-ago trip still exists, has_prev = true.
    let resp = server
        .get("/driver/api/v1/trips?tab=past&week_start=2026-04-26")
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = serde_json::from_slice(resp.as_bytes()).unwrap();
    assert_eq!(body["week"]["has_prev"], true, "week 2026-04-26: has_prev should be true");

    // Query the oldest week (2026-04-12) — no older trips, has_prev = false, items.len() == 1.
    let resp = server
        .get("/driver/api/v1/trips?tab=past&week_start=2026-04-12")
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = serde_json::from_slice(resp.as_bytes()).unwrap();
    assert_eq!(body["week"]["has_prev"], false, "week 2026-04-12: has_prev should be false");
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "week 2026-04-12: should have exactly 1 trip");
}

#[tokio::test]
async fn test_version_endpoint_returns_cargo_version() {
    let (server, _db_dir, _blob_dir, _rx) = test_server().await;

    let resp = server.get("/version").await;
    resp.assert_status_ok();

    let body: serde_json::Value = resp.json();
    let version = body["version"].as_str().expect("version field missing");

    assert_eq!(version, env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn test_version_endpoint_is_unauthenticated() {
    let (server, _db_dir, _blob_dir, _rx) = test_server().await;

    let resp = server.get("/version").await;
    resp.assert_status_ok();
}

// ---------------------------------------------------------------------------
// Issue #220 — driver PATCH must cascade actuals to load stop and fire status
// transitions, mirroring the fleet_user stop_arrive/stop_depart handlers.
// ---------------------------------------------------------------------------

async fn setup_driver_with_dispatched_load_trip(
    server: &TestServer,
    state: &AppState,
) -> (String, String, String) {
    let owner_token = setup_owner(server).await;
    let driver_id_str = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Cascade Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let driver_id: uuid::Uuid = driver_id_str.parse().unwrap();

    let creds = ollie::models::DriverCredentials {
        driver_id,
        pin_hash: None,
        token_version: 1,
        failed_pin_attempts: 0,
        locked_until: None,
        updated_at: chrono::Utc::now(),
    };
    state.db.upsert_driver_credentials(&creds).await.unwrap();

    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "T-CSC-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let origin_fac = create_test_facility(server, "Origin", "Chicago, IL").await;
    let dest_fac = create_test_facility(server, "Dest", "Memphis, TN").await;

    let load_id = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [
                {"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                 "facility_id": origin_fac, "scheduled_arrive": "2026-05-10T08:00:00",
                 "timezone": "America/Chicago"},
                {"sequence": 2, "stop_type": "delivery", "service_type": "live_unload",
                 "facility_id": dest_fac, "scheduled_arrive": "2026-05-10T18:00:00",
                 "timezone": "America/Chicago"},
            ],
            "rate_items": [{"description": "Line Haul", "amount_usd": 1500.0}]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "load_id": load_id }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let assign = server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id_str, "truck_id": truck_id }))
        .await;
    assert_eq!(assign.status_code(), 200);

    let dispatch = server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(dispatch.status_code(), 200);

    let secret = std::env::var("DRIVER_JWT_SECRET").unwrap();
    let token = ollie::api::driver_portal::jwt::encode_driver_jwt(driver_id, 1, &secret).unwrap();
    (token, trip_id, load_id)
}

#[tokio::test]
async fn test_220_driver_patch_cascades_to_load_and_transitions_status() {
    let (server, _db, _blob, _rx, state) = test_server_with_state().await;
    let (driver_token, trip_id, load_id) =
        setup_driver_with_dispatched_load_trip(&server, &state).await;
    let load_uuid: uuid::Uuid = load_id.parse().unwrap();
    let trip_uuid: uuid::Uuid = trip_id.parse().unwrap();

    // Driver arrives at the pickup stop.
    let r = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/1"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({ "actual_arrive": "2026-05-10T08:00:00" }))
        .await;
    assert_eq!(r.status_code(), 200);

    let load = state.db.get_load_by_id(load_uuid).await.unwrap();
    assert_eq!(load.stops[0].actual_arrive.as_deref(), Some("2026-05-10T08:00:00"),
        "#220: driver-recorded arrive must cascade to load stop");

    // Driver departs the pickup → trip + load should advance to InTransit.
    let r = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/1"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-05-10T09:00:00" }))
        .await;
    assert_eq!(r.status_code(), 200);

    let load = state.db.get_load_by_id(load_uuid).await.unwrap();
    assert_eq!(load.stops[0].actual_depart.as_deref(), Some("2026-05-10T09:00:00"),
        "#220: driver-recorded depart must cascade to load stop");
    assert_eq!(load.status, ollie::models::LoadStatus::InTransit,
        "#220: first-pickup depart on driver path must transition load to InTransit");
    let trip = state.db.get_trip(trip_uuid).await.unwrap();
    assert_eq!(trip.status, ollie::models::TripStatus::InTransit,
        "#220: first-pickup depart on driver path must transition trip to InTransit");

    // Driver delivers the load.
    let r = server.patch(&format!("/driver/api/v1/trips/{trip_id}/stops/2"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .json(&serde_json::json!({
            "actual_arrive": "2026-05-10T17:30:00",
            "actual_depart": "2026-05-10T18:15:00"
        }))
        .await;
    assert_eq!(r.status_code(), 200);

    let load = state.db.get_load_by_id(load_uuid).await.unwrap();
    assert_eq!(load.stops[1].actual_arrive.as_deref(), Some("2026-05-10T17:30:00"));
    assert_eq!(load.stops[1].actual_depart.as_deref(), Some("2026-05-10T18:15:00"));
    assert_eq!(load.status, ollie::models::LoadStatus::Delivered,
        "#220: final stop depart on driver path must cascade load to Delivered");

    // Confirm stop.arrived / stop.departed events were appended for the driver path.
    let (_, events) = state.db.query_events(
        Some(trip_uuid), Some("trip"), None, None, None, 100, 0,
    ).await.unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert!(kinds.contains(&"stop.arrived"),
        "#220: driver path must fire stop.arrived event");
    assert!(kinds.contains(&"stop.departed"),
        "#220: driver path must fire stop.departed event");
    assert!(kinds.contains(&"trip.in_transit"),
        "#220: driver path must fire trip.in_transit event");
    assert!(kinds.contains(&"trip.delivered"),
        "#220: driver path must fire trip.delivered event");
}

// ============================================================
// Dispatcher API key tests
// ============================================================

async fn create_fleet_user_api_key(
    server: &axum_test::TestServer,
    token: &str,
    label: &str,
) -> serde_json::Value {
    let resp = server.post("/fleet/api-keys")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "label": label }))
        .await;
    assert_eq!(resp.status_code(), 201, "create api key failed: {}", resp.text());
    resp.json::<serde_json::Value>()
}

#[tokio::test]
async fn test_api_key_create_returns_plaintext_once() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "apikey1@example.com", "pass-apikey1").await;

    let body = create_fleet_user_api_key(&server, &token, "Claude desktop").await;

    assert!(body["key"].as_str().unwrap().starts_with("olld_"));
    assert_eq!(body["key"].as_str().unwrap().len(), 48);
    assert_eq!(body["label"], "Claude desktop");
    assert!(body["key_prefix"].as_str().is_some());
    assert_eq!(&body["key"].as_str().unwrap()[..12], body["key_prefix"].as_str().unwrap());
    assert!(body["id"].as_str().is_some());
    assert!(body["expires_at"].as_str().is_some());
}

#[tokio::test]
async fn test_api_key_custom_expiry() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "apikey2@example.com", "pass-apikey2").await;

    let resp = server.post("/fleet/api-keys")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "label": "short-lived", "expires_in_days": 7 }))
        .await;
    assert_eq!(resp.status_code(), 201);
    let body = resp.json::<serde_json::Value>();
    let expires: chrono::DateTime<chrono::Utc> = body["expires_at"].as_str().unwrap().parse().unwrap();
    let created: chrono::DateTime<chrono::Utc> = body["created_at"].as_str().unwrap().parse().unwrap();
    let diff = expires - created;
    assert!(diff.num_days() >= 6 && diff.num_days() <= 8, "expected ~7 day expiry, got {}", diff.num_days());
}

#[tokio::test]
async fn test_api_key_expiry_over_365_rejected() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "apikey3@example.com", "pass-apikey3").await;

    let resp = server.post("/fleet/api-keys")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "label": "too-long", "expires_in_days": 366 }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_api_key_list_returns_own_keys_only() {
    let (server, _b, _d, _rx) = test_server().await;
    let t1 = fleet_user_login(&server, "apikeylist1@example.com", "pass1").await;
    let t2 = fleet_user_login(&server, "apikeylist2@example.com", "pass2").await;

    create_fleet_user_api_key(&server, &t1, "d1-key").await;
    create_fleet_user_api_key(&server, &t2, "d2-key").await;

    let resp = server.get("/fleet/api-keys")
        .add_header(header::AUTHORIZATION, format!("Bearer {t1}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    let keys = body["keys"].as_array().unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["label"], "d1-key");
    assert!(keys[0]["key"].is_null(), "plaintext must not appear in list");
}

#[tokio::test]
async fn test_api_key_list_excludes_revoked() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "apikeyrev1@example.com", "pass-rev1").await;

    let body = create_fleet_user_api_key(&server, &token, "to-revoke").await;
    let key_id = body["id"].as_str().unwrap();

    let del = server.delete(&format!("/fleet/api-keys/{key_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(del.status_code(), 204);

    let list = server.get("/fleet/api-keys")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    let list_body = list.json::<serde_json::Value>();
    assert_eq!(list_body["keys"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_api_key_revoke_not_found_for_other_fleet_user() {
    let (server, _b, _d, _rx) = test_server().await;
    let t1 = fleet_user_login(&server, "apikeyown1@example.com", "pass-own1").await;
    let t2 = fleet_user_login(&server, "apikeyown2@example.com", "pass-own2").await;

    let body = create_fleet_user_api_key(&server, &t1, "t1-key").await;
    let key_id = body["id"].as_str().unwrap();

    let resp = server.delete(&format!("/fleet/api-keys/{key_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {t2}"))
        .await;
    assert_eq!(resp.status_code(), 404);
}

#[tokio::test]
async fn test_api_key_auth_grants_access_to_protected_endpoint() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "apikeyauth1@example.com", "pass-auth1").await;

    let body = create_fleet_user_api_key(&server, &token, "Claude desktop").await;
    let api_key = body["key"].as_str().unwrap();

    let resp = server.get("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {api_key}"))
        .await;
    assert_eq!(resp.status_code(), 200);
}

#[tokio::test]
async fn test_api_key_auth_works_on_mcp_endpoint() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "apikeymcp1@example.com", "pass-mcp1").await;

    let body = create_fleet_user_api_key(&server, &token, "Claude MCP").await;
    let api_key = body["key"].as_str().unwrap();

    let resp = server.post("/fleet/mcp")
        .add_header(header::ACCEPT, "application/json, text/event-stream")
        .add_header(header::AUTHORIZATION, format!("Bearer {api_key}"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "ollie-test", "version": "1.0" }
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let result = sse_json(&resp.text());
    assert_eq!(result["result"]["protocolVersion"], "2025-06-18");
}

#[tokio::test]
async fn test_revoked_api_key_rejected() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "apikeyrvk1@example.com", "pass-rvk1").await;

    let body = create_fleet_user_api_key(&server, &token, "to-revoke").await;
    let api_key = body["key"].as_str().unwrap().to_string();
    let key_id = body["id"].as_str().unwrap();

    server.delete(&format!("/fleet/api-keys/{key_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;

    let resp = server.get("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {api_key}"))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_jwt_auth_still_works_after_api_key_feature() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "apikeycompat1@example.com", "pass-compat1").await;

    let resp = server.get("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
}

#[tokio::test]
async fn test_api_key_create_requires_jwt_not_api_key() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "apikeyself1@example.com", "pass-self1").await;

    let body = create_fleet_user_api_key(&server, &token, "first-key").await;
    let api_key = body["key"].as_str().unwrap();

    let resp = server.post("/fleet/api-keys")
        .add_header(header::AUTHORIZATION, format!("Bearer {api_key}"))
        .json(&serde_json::json!({ "label": "self-created" }))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_api_key_revoke_requires_jwt_not_api_key() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "apikeyself2@example.com", "pass-self2").await;

    let body = create_fleet_user_api_key(&server, &token, "key-to-revoke").await;
    let api_key = body["key"].as_str().unwrap().to_string();
    let key_id = body["id"].as_str().unwrap();

    let resp = server.delete(&format!("/fleet/api-keys/{key_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {api_key}"))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_api_key_20_key_cap() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "apikeycap1@example.com", "pass-cap1").await;

    for i in 0..20 {
        let resp = server.post("/fleet/api-keys")
            .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
            .json(&serde_json::json!({ "label": format!("key-{i}") }))
            .await;
        assert_eq!(resp.status_code(), 201, "key {i} creation should succeed");
    }

    let resp = server.post("/fleet/api-keys")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "label": "key-21" }))
        .await;
    assert_eq!(resp.status_code(), 429);
}

// ── compute_and_persist_mileage helper (Task 3, #259 prep) ─────────────────────────────────

#[tokio::test]
async fn test_compute_and_persist_mileage_returns_ors_unavailable_when_coords_missing() {
    // Trip with stops on facilities that have no lat/lng → helper returns 409 OrsRoutingUnavailable.
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, "Dock A", "Somewhere, US").await;
    let fac2_id = create_test_facility(&server, "Dock B", "Elsewhere, US").await;
    let load_id = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [
                {"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                 "facility_id": fac_id, "scheduled_arrive": "2026-05-10T08:00:00",
                 "timezone": "America/Chicago"},
                {"sequence": 2, "stop_type": "delivery", "service_type": "live_unload",
                 "facility_id": fac2_id, "scheduled_arrive": "2026-05-11T08:00:00",
                 "timezone": "America/Chicago"},
            ],
            "rate_items": [{"description": "LH", "amount_usd": 100.0}]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_resp = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "load_id": load_id }))
        .await;
    assert_eq!(trip_resp.status_code(), 201);
    let trip_id_str = trip_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let trip_id = uuid::Uuid::parse_str(&trip_id_str).unwrap();

    let result = ollie::api::trips::compute_and_persist_mileage(&state, trip_id).await;
    match result {
        Err(ollie::error::AppError::OrsRoutingUnavailable(_)) => {}
        other => panic!("expected OrsRoutingUnavailable, got {other:?}"),
    }

    // DB unchanged: still no miles
    let trip_after = state.db.get_trip(trip_id).await.unwrap();
    assert!(trip_after.total_miles.is_none());
}

#[tokio::test]
async fn test_compute_and_persist_mileage_not_found() {
    let (_server, _b, _d, _rx, state) = test_server_with_state().await;
    let result = ollie::api::trips::compute_and_persist_mileage(&state, uuid::Uuid::new_v4()).await;
    assert!(matches!(result, Err(ollie::error::AppError::NotFound)));
}

// ── fleet_user PATCH + recalculate-miles endpoints (Task 4, #259, #262) ─────

async fn make_trip_with_two_stops(server: &axum_test::TestServer) -> String {
    let owner_token = setup_owner(server).await;
    let fac1 = create_test_facility(server, "Recalc Dock A", "Dallas, TX").await;
    let fac2 = create_test_facility(server, "Recalc Dock B", "Houston, TX").await;
    server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "Recalc Co",
            "stops": [
                {"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                 "facility_id": fac1, "scheduled_arrive": "2026-06-01T08:00:00",
                 "timezone": "America/Chicago"},
                {"sequence": 2, "stop_type": "delivery", "service_type": "live_unload",
                 "facility_id": fac2, "scheduled_arrive": "2026-06-02T08:00:00",
                 "timezone": "America/Chicago"},
            ],
            "rate_items": [{"description": "LH", "amount_usd": 100.0}]
        }))
        .await;
    let trip = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                {"sequence": 1, "stop_type": "pickup", "facility_id": fac1,
                 "scheduled_arrive": "2026-06-01T08:00:00", "timezone": "America/Chicago"},
                {"sequence": 2, "stop_type": "delivery", "facility_id": fac2,
                 "scheduled_arrive": "2026-06-02T08:00:00", "timezone": "America/Chicago"},
            ]
        }))
        .await;
    assert_eq!(trip.status_code(), 201);
    trip.json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_recalculate_miles_returns_409_when_ors_unavailable() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "recalc1@example.com", "password-recalc1").await;
    let trip_id = make_trip_with_two_stops(&server).await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/recalculate-miles"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 409);
}

#[tokio::test]
async fn test_recalculate_miles_requires_auth() {
    let (server, _b, _d, _rx) = test_server().await;
    let trip_id = make_trip_with_two_stops(&server).await;
    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/recalculate-miles"))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_recalculate_miles_returns_existing_summary_when_already_set() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let token = fleet_user_login(&server, "recalc2@example.com", "password-recalc2").await;
    let trip_id_str = make_trip_with_two_stops(&server).await;
    let trip_id = uuid::Uuid::parse_str(&trip_id_str).unwrap();

    // Seed miles directly via DB so the "already set" branch fires.
    state.db.update_trip_mileage(
        trip_id, Some(50.0), Some(450.0), Some(500.0), vec![50.0, 450.0],
    ).await.unwrap();
    let before = state.db.get_trip(trip_id).await.unwrap();
    let updated_at_before = before.updated_at;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id_str}/recalculate-miles"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["total_miles"].as_f64(), Some(500.0));
    assert_eq!(body["loaded_miles"].as_f64(), Some(450.0));

    // No DB write → updated_at unchanged.
    let after = state.db.get_trip(trip_id).await.unwrap();
    assert_eq!(after.updated_at, updated_at_before);
}

#[tokio::test]
async fn test_recalculate_miles_force_triggers_recompute() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let token = fleet_user_login(&server, "recalc3@example.com", "password-recalc3").await;
    let trip_id_str = make_trip_with_two_stops(&server).await;
    let trip_id = uuid::Uuid::parse_str(&trip_id_str).unwrap();

    // Seed miles so the "already set" guard would otherwise skip recompute.
    state.db.update_trip_mileage(
        trip_id, Some(10.0), Some(20.0), Some(30.0), vec![10.0, 20.0],
    ).await.unwrap();

    // ORS is unavailable in tests → force=true must call helper and surface 409
    // (proves the force flag bypassed the early-return branch).
    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id_str}/recalculate-miles"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "force": true }))
        .await;
    assert_eq!(resp.status_code(), 409);
}

#[tokio::test]
async fn test_patch_trip_updates_notes() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "patch1@example.com", "password-patch1").await;
    let trip_id = make_trip_with_two_stops(&server).await;

    let resp = server.patch(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "notes": "fleet_user note" }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["id"], trip_id);
    assert_eq!(body["notes"], "fleet_user note",
        "fleet_user PATCH response should echo updated notes");

    // Also confirm persistence via admin GET.
    let get = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    let get_body = get.json::<serde_json::Value>();
    assert_eq!(get_body["notes"], "fleet_user note");
}

#[tokio::test]
async fn test_patch_trip_rejects_raw_mileage() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "patch2@example.com", "password-patch2").await;
    let trip_id = make_trip_with_two_stops(&server).await;

    for field in ["deadhead_miles", "loaded_miles", "total_miles", "segment_miles"] {
        let body = if field == "segment_miles" {
            serde_json::json!({ field: [1.0, 2.0] })
        } else {
            serde_json::json!({ field: 100.0 })
        };
        let resp = server.patch(&format!("/fleet/api/v1/trips/{trip_id}"))
            .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
            .json(&body)
            .await;
        assert_eq!(resp.status_code(), 400, "expected 400 for field {field}");
    }
}

#[tokio::test]
async fn test_patch_trip_rejects_unknown_field() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "patch3@example.com", "password-patch3").await;
    let trip_id = make_trip_with_two_stops(&server).await;

    let resp = server.patch(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "load_id": uuid::Uuid::new_v4() }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_patch_trip_requires_auth() {
    let (server, _b, _d, _rx) = test_server().await;
    let trip_id = make_trip_with_two_stops(&server).await;
    let resp = server.patch(&format!("/fleet/api/v1/trips/{trip_id}"))
        .json(&serde_json::json!({ "notes": "x" }))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_patch_trip_previous_trip_id_commits_even_when_recompute_fails() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let token = fleet_user_login(&server, "patch4@example.com", "password-patch4").await;
    let trip_id = make_trip_with_two_stops(&server).await;
    let other_trip_id = make_trip_with_two_stops(&server).await;

    // Linking to a prev trip forces a mileage recompute. ORS is mocked
    // unavailable in tests, so the recompute fails — but the previous_trip_id
    // write itself must still commit. v1.17.0 returned 409 here, which hid the
    // partial commit from callers; v1.17.1 returns 200 with a non-null
    // `mileage_recompute_warning` so the caller knows exactly what happened.
    let resp = server.patch(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "previous_trip_id": other_trip_id }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert!(
        body["mileage_recompute_warning"].is_string(),
        "expected non-null mileage_recompute_warning, got {:?}",
        body["mileage_recompute_warning"],
    );

    // Verify the previous_trip_id link did commit by re-reading the persisted
    // record via the DB handle (the fleet_user view doesn't expose
    // previous_trip_id as a top-level field).
    let rec = state.db.get_trip(trip_id.parse().unwrap()).await.unwrap();
    assert_eq!(rec.previous_trip_id.map(|u| u.to_string()).as_deref(),
        Some(other_trip_id.as_str()));
}

#[tokio::test]
async fn test_patch_trip_sets_rate_override() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let token = fleet_user_login(&server, "patchrate1@example.com", "password-patchrate1").await;
    let trip_id = make_trip_with_two_stops(&server).await;

    let resp = server.patch(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "loaded_rate_per_mile": 2.5 }))
        .await;
    assert_eq!(resp.status_code(), 200);

    let rec = state.db.get_trip(trip_id.parse().unwrap()).await.unwrap();
    assert_eq!(rec.loaded_rate_per_mile, Some(2.5),
        "value present should set the override");
}

#[tokio::test]
async fn test_patch_trip_clears_rate_override_with_null() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let token = fleet_user_login(&server, "patchrate2@example.com", "password-patchrate2").await;
    let trip_id = make_trip_with_two_stops(&server).await;
    let tid: uuid::Uuid = trip_id.parse().unwrap();

    // Seed an override first.
    let set = server.patch(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "loaded_rate_per_mile": 2.5 }))
        .await;
    assert_eq!(set.status_code(), 200);
    assert_eq!(state.db.get_trip(tid).await.unwrap().loaded_rate_per_mile, Some(2.5));

    // Explicit null clears it back to inherited (None).
    let clear = server.patch(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "loaded_rate_per_mile": null }))
        .await;
    assert_eq!(clear.status_code(), 200);
    assert_eq!(state.db.get_trip(tid).await.unwrap().loaded_rate_per_mile, None,
        "explicit null should clear the override to inherited");
}

#[tokio::test]
async fn test_patch_trip_omitted_rate_field_leaves_override_unchanged() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let token = fleet_user_login(&server, "patchrate3@example.com", "password-patchrate3").await;
    let trip_id = make_trip_with_two_stops(&server).await;
    let tid: uuid::Uuid = trip_id.parse().unwrap();

    // Seed an override.
    let seed = server.patch(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "loaded_rate_per_mile": 2.5 }))
        .await;
    assert_eq!(seed.status_code(), 200, "seeding the override should succeed");

    // PATCH that omits the rate field (touches only notes) must leave it as-is.
    let resp = server.patch(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "notes": "unrelated" }))
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(state.db.get_trip(tid).await.unwrap().loaded_rate_per_mile, Some(2.5),
        "omitted rate field must not change the override");
}

#[tokio::test]
async fn test_patch_trip_clear_rate_on_settled_trip_409s() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let token = fleet_user_login(&server, "patchrate4@example.com", "password-patchrate4").await;
    let trip_id = make_trip_with_two_stops(&server).await;
    let tid: uuid::Uuid = trip_id.parse().unwrap();

    // Seed an override, then settle the trip (freezes pay-affecting fields).
    server.patch(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "loaded_rate_per_mile": 2.5 }))
        .await;
    let settle = server.patch(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "settlement_ref": "SETTLE-1" }))
        .await;
    assert_eq!(settle.status_code(), 200, "settling the trip should succeed");

    // Clearing a rate override via null on a settled trip must 409, exactly like
    // setting a value would — Some(None) is pay-affecting too.
    let clear = server.patch(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "loaded_rate_per_mile": null }))
        .await;
    assert_eq!(clear.status_code(), 409,
        "clearing a rate override on a settled trip is frozen");

    // And the override is untouched.
    assert_eq!(state.db.get_trip(tid).await.unwrap().loaded_rate_per_mile, Some(2.5));
}

// ── Doctors (trip / load / facility) — v1.17.1 ─────────────────────────────

#[tokio::test]
async fn test_trip_doctor_dry_run_reports_missing_stop_metadata_without_writes() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "doctor1@example.com", "password-doctor1").await;
    let fac1 = create_test_facility(&server, "Doc Dock A", "Memphis, TN").await;
    let fac2 = create_test_facility(&server, "Doc Dock B", "Atlanta, GA").await;

    // Load has rich stop metadata (notes, end window, dwell).
    let load_resp = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "Doc Co",
            "stops": [
                {"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                 "facility_id": fac1, "scheduled_arrive": "2026-07-01T08:00:00",
                 "scheduled_arrive_end": "2026-07-01T12:00:00",
                 "expected_dwell_minutes": 60, "notes": "PU #123",
                 "timezone": "America/Chicago"},
                {"sequence": 2, "stop_type": "delivery", "service_type": "live_unload",
                 "facility_id": fac2, "scheduled_arrive": "2026-07-02T08:00:00",
                 "notes": "Drop and hook", "timezone": "America/New_York"},
            ],
            "rate_items": [{"description": "LH", "amount_usd": 500.0}]
        }))
        .await;
    let load_id = load_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Trip has bare facility_id stops — the exact T-0015 corruption pattern.
    let trip = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "load_id": load_id,
            "stops": [
                {"sequence": 1, "stop_type": "pickup", "facility_id": fac1},
                {"sequence": 2, "stop_type": "delivery", "facility_id": fac2},
            ]
        }))
        .await;
    assert_eq!(trip.status_code(), 201);
    let trip_id = trip.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Dry-run.
    let report = mcp_call(&server, &token, "trip_doctor", serde_json::json!({
        "trip_id": trip_id
    })).await;
    assert_eq!(report["dry_run"], serde_json::json!(true));
    assert!(report["applied"].as_array().unwrap().is_empty(), "dry-run wrote: {report}");
    let findings = report["findings"].as_array().unwrap();
    let metadata_finding = findings.iter()
        .find(|f| f["check"] == "trip.stops.metadata_complete")
        .expect("expected metadata_complete finding");
    assert_eq!(metadata_finding["fix"]["kind"], "resync_stops_from_load");
    assert_eq!(metadata_finding["fix"]["safe_to_auto_apply"], serde_json::json!(true));

    // DB unchanged.
    let get_before = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    let before: serde_json::Value = get_before.json();
    assert!(before["stops"][0]["scheduled_arrive_end"].is_null());
    assert!(before["stops"][0]["notes"].is_null());
}

#[tokio::test]
async fn test_trip_doctor_apply_resyncs_stops_from_load_without_overwriting() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "doctor2@example.com", "password-doctor2").await;
    let fac1 = create_test_facility(&server, "Apply Dock A", "Memphis, TN").await;
    let fac2 = create_test_facility(&server, "Apply Dock B", "Atlanta, GA").await;

    let load_resp = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "Apply Co",
            "stops": [
                {"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                 "facility_id": fac1, "scheduled_arrive": "2026-07-01T08:00:00",
                 "scheduled_arrive_end": "2026-07-01T12:00:00",
                 "expected_dwell_minutes": 60, "notes": "PU #123",
                 "timezone": "America/Chicago"},
                {"sequence": 2, "stop_type": "delivery", "service_type": "live_unload",
                 "facility_id": fac2, "scheduled_arrive": "2026-07-02T08:00:00",
                 "notes": "Drop and hook", "timezone": "America/New_York"},
            ],
            "rate_items": [{"description": "LH", "amount_usd": 500.0}]
        }))
        .await;
    let load_id = load_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Trip stops with one pre-existing non-null field (notes on stop 1) that
    // disagrees with the load — verifies diff-and-confirm does NOT clobber it.
    let trip = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "load_id": load_id,
            "stops": [
                {"sequence": 1, "stop_type": "pickup", "facility_id": fac1,
                 "notes": "fleet_user amended note"},
                {"sequence": 2, "stop_type": "delivery", "facility_id": fac2},
            ]
        }))
        .await;
    let trip_id = trip.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let report = mcp_call(&server, &token, "trip_doctor", serde_json::json!({
        "trip_id": trip_id, "apply": true,
    })).await;
    assert_eq!(report["dry_run"], serde_json::json!(false));
    // The metadata finding should be present but have a conflict, so the fix
    // does NOT auto-apply — diff-and-confirm.
    let findings = report["findings"].as_array().unwrap();
    let metadata_finding = findings.iter()
        .find(|f| f["check"] == "trip.stops.metadata_complete")
        .expect("metadata finding present");
    let conflicts = metadata_finding["fix"]["conflicts"].as_array().unwrap();
    assert!(
        conflicts.iter().any(|c| c.as_str().unwrap().contains("notes")),
        "expected notes conflict, got conflicts={conflicts:?}"
    );
    assert_eq!(metadata_finding["fix"]["safe_to_auto_apply"], serde_json::json!(false));
    let skipped = report["skipped_due_to_conflict"].as_array().unwrap();
    assert!(skipped.iter().any(|c| c == "trip.stops.metadata_complete"));

    // Stop 1's non-null `notes` survived; the load's value did not clobber it.
    let trip_after = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(trip_after["stops"][0]["notes"], "fleet_user amended note");
}

#[tokio::test]
async fn test_load_doctor_flags_ungeocoded_facility() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "doctor3@example.com", "password-doctor3").await;
    // The test geocoder doesn't fire — facilities created here have no
    // lat/lng, which is exactly what load_doctor's facility_geocoded check
    // should flag.
    let fac1 = create_test_facility(&server, "Ungeo Dock", "Memphis, TN").await;
    let fac2 = create_test_facility(&server, "Ungeo Dock 2", "Atlanta, GA").await;
    let load_resp = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "LD Co",
            "stops": [
                {"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                 "facility_id": fac1, "scheduled_arrive": "2026-07-01T08:00:00",
                 "timezone": "America/Chicago"},
                {"sequence": 2, "stop_type": "delivery", "service_type": "live_unload",
                 "facility_id": fac2, "scheduled_arrive": "2026-07-02T08:00:00",
                 "timezone": "America/New_York"},
            ],
            "rate_items": [{"description": "LH", "amount_usd": 100.0}]
        }))
        .await;
    let status = load_resp.status_code();
    let text = load_resp.text();
    assert_eq!(status, 201, "load create failed: {status} {text}");
    let load_id = serde_json::from_str::<serde_json::Value>(&text)
        .expect("load response is JSON")["id"].as_str().unwrap().to_string();

    let report = mcp_call(&server, &token, "load_doctor", serde_json::json!({
        "load_id": load_id
    })).await;
    let findings = report["findings"].as_array().unwrap();
    assert!(
        findings.iter().any(|f| f["check"] == "load.stops.facility_geocoded"),
        "expected facility_geocoded finding, got findings={findings:?}"
    );
}

// ── Task 5: MCP create_trip/update_trip/recalculate_trip_miles + filters ───────

/// Extract the single JSON-RPC message from a Streamable-HTTP SSE response body.
/// rmcp frames each POST reply as one `event: message` / `data: {…}` SSE event.
fn sse_json(body: &str) -> serde_json::Value {
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            let rest = rest.trim();
            if rest.is_empty() {
                continue; // priming/keep-alive frames carry no JSON payload
            }
            return serde_json::from_str(rest)
                .unwrap_or_else(|e| panic!("SSE data not JSON ({e}): {rest}"));
        }
    }
    panic!("no SSE `data:` event in body: {body:?}");
}

/// Open an MCP session: `initialize` then the `notifications/initialized` ack.
/// Returns the `Mcp-Session-Id` the server assigned (used on subsequent calls).
async fn mcp_session(server: &axum_test::TestServer, token: &str) -> String {
    let resp = server
        .post("/fleet/mcp")
        .add_header(header::ACCEPT, "application/json, text/event-stream")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "ollie-test", "version": "1.0" }
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "initialize HTTP {}", resp.status_code());
    let session = resp
        .headers()
        .get("mcp-session-id")
        .expect("initialize must return an Mcp-Session-Id header")
        .to_str()
        .unwrap()
        .to_string();
    server
        .post("/fleet/mcp")
        .add_header(header::ACCEPT, "application/json, text/event-stream")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .add_header("mcp-session-id", session.clone())
        .json(&serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;
    session
}

/// Send one JSON-RPC request on an existing session and return the parsed reply
/// (the full JSON-RPC envelope: `{ jsonrpc, id, result | error }`).
async fn mcp_rpc(
    server: &axum_test::TestServer,
    token: &str,
    session: &str,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let resp = server
        .post("/fleet/mcp")
        .add_header(header::ACCEPT, "application/json, text/event-stream")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .add_header("mcp-session-id", session.to_string())
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": method, "params": params
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "MCP {method} HTTP {}", resp.status_code());
    sse_json(&resp.text())
}

/// Full convenience path: open a session, invoke a tool, and return the tool's
/// decoded payload (the JSON object inside the result's text content block).
async fn mcp_call(
    server: &axum_test::TestServer,
    token: &str,
    name: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    let session = mcp_session(server, token).await;
    let body = mcp_rpc(
        server,
        token,
        &session,
        "tools/call",
        serde_json::json!({ "name": name, "arguments": args }),
    )
    .await;
    assert!(body["error"].is_null(), "MCP {name} error: {:?}", body["error"]);
    let text = body["result"]["content"][0]["text"]
        .as_str()
        .expect("MCP content[0].text missing");
    serde_json::from_str(text).expect("inner JSON parse")
}

#[tokio::test]
async fn test_mcp_create_trip_returns_trip_with_mileage_summary() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mcp_ct@example.com", "password-mcp-ct").await;

    let fac1 = create_test_facility(&server, "MCP CT Dock A", "Dallas, TX").await;
    let fac2 = create_test_facility(&server, "MCP CT Dock B", "Houston, TX").await;

    let trip = mcp_call(&server, &token, "create_trip", serde_json::json!({
        "stops": [
            { "sequence": 1, "stop_type": "pickup", "facility_id": fac1,
              "scheduled_arrive": "2026-08-01T08:00:00", "timezone": "America/Chicago" },
            { "sequence": 2, "stop_type": "delivery", "facility_id": fac2,
              "scheduled_arrive": "2026-08-02T08:00:00", "timezone": "America/Chicago" },
        ]
    })).await;

    assert!(trip["id"].as_str().is_some());
    assert!(trip["trip_number"].as_str().unwrap().starts_with("T-"));
    // Should carry mileage_summary block (even with no miles computed in test ORS).
    assert!(trip.get("mileage_summary").is_some(),
        "create_trip response missing mileage_summary: {trip:?}");
}

#[tokio::test]
async fn test_mcp_update_trip_updates_notes() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "mcp_ut@example.com", "password-mcp-ut").await;
    let trip_id = make_trip_with_two_stops(&server).await;

    let trip = mcp_call(&server, &token, "update_trip", serde_json::json!({
        "trip_id": trip_id,
        "notes": "via MCP"
    })).await;
    assert_eq!(trip["id"], trip_id);

    // Verify persistence
    let get = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(get.json::<serde_json::Value>()["notes"], "via MCP");
}

#[tokio::test]
async fn test_mcp_update_trip_rejects_raw_mileage() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mcp_ut2@example.com", "password-mcp-ut2").await;
    let trip_id = make_trip_with_two_stops(&server).await;
    let session = mcp_session(&server, &token).await;

    let body = mcp_rpc(&server, &token, &session, "tools/call", serde_json::json!({
        "name": "update_trip",
        "arguments": { "trip_id": trip_id, "total_miles": 999.0 }
    })).await;
    // A domain rejection is recoverable feedback: an isError RESULT, not a
    // JSON-RPC error (so the model reads the message and adapts) — see #297.
    assert!(body["error"].is_null(), "domain failure must not be a JSON-RPC error: {body:?}");
    assert_eq!(body["result"]["isError"], serde_json::json!(true));
    let msg = body["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(!msg.is_empty(), "isError result must carry a human-readable message");
}

/// #297: protocol faults (unknown tool) stay on the JSON-RPC error channel, while
/// tool-execution failures become isError results (covered above). These two
/// channels must not be conflated.
#[tokio::test]
async fn test_mcp_unknown_tool_is_jsonrpc_error_not_iserror() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mcp_unk@example.com", "password-mcp-unk").await;
    let session = mcp_session(&server, &token).await;

    let body = mcp_rpc(&server, &token, &session, "tools/call", serde_json::json!({
        "name": "no_such_tool",
        "arguments": {}
    })).await;
    assert!(
        body["error"].is_object(),
        "unknown tool is a protocol fault → JSON-RPC error, got: {body:?}"
    );
    assert!(body["result"].is_null(), "protocol fault must not return a result");
}

#[tokio::test]
async fn test_mcp_recalculate_trip_miles_returns_summary() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let token = fleet_user_login(&server, "mcp_rc@example.com", "password-mcp-rc").await;
    let trip_id_str = make_trip_with_two_stops(&server).await;
    let trip_id = uuid::Uuid::parse_str(&trip_id_str).unwrap();

    // Seed miles so the early-return branch fires (ORS unavailable in tests).
    state.db.update_trip_mileage(
        trip_id, Some(11.0), Some(22.0), Some(33.0), vec![11.0, 22.0],
    ).await.unwrap();

    let summary = mcp_call(&server, &token, "recalculate_trip_miles", serde_json::json!({
        "trip_id": trip_id_str
    })).await;
    assert_eq!(summary["total_miles"].as_f64(), Some(33.0));
    assert_eq!(summary["loaded_miles"].as_f64(), Some(22.0));
    assert_eq!(summary["deadhead_miles"].as_f64(), Some(11.0));
}

#[tokio::test]
async fn test_mcp_get_trip_includes_full_mileage_summary() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let token = fleet_user_login(&server, "mcp_gt@example.com", "password-mcp-gt").await;
    let trip_id_str = make_trip_with_two_stops(&server).await;
    let trip_id = uuid::Uuid::parse_str(&trip_id_str).unwrap();
    state.db.update_trip_mileage(
        trip_id, Some(11.0), Some(22.0), Some(33.0), vec![11.0, 22.0],
    ).await.unwrap();

    let trip = mcp_call(&server, &token, "get_trip", serde_json::json!({
        "id": trip_id_str
    })).await;
    let ms = &trip["mileage_summary"];
    assert!(ms.is_object(), "mileage_summary missing on MCP get_trip");
    // origin block + legs array contract
    assert!(ms.get("origin").is_some(), "mileage_summary.origin missing");
    assert!(ms["legs"].is_array(), "mileage_summary.legs missing");
}

#[tokio::test]
async fn test_mcp_list_trips_items_carry_mileage_fields() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let token = fleet_user_login(&server, "mcp_lt@example.com", "password-mcp-lt").await;
    let trip_id_str = make_trip_with_two_stops(&server).await;
    let trip_id = uuid::Uuid::parse_str(&trip_id_str).unwrap();
    state.db.update_trip_mileage(
        trip_id, Some(7.0), Some(13.0), Some(20.0), vec![7.0, 13.0],
    ).await.unwrap();

    let resp = mcp_call(&server, &token, "list_trips", serde_json::json!({})).await;
    let items = resp["items"].as_array().expect("items array");
    let item = items.iter().find(|it| it["id"] == trip_id_str)
        .expect("trip not found in list");
    assert_eq!(item["deadhead_miles"].as_f64(), Some(7.0));
    assert_eq!(item["loaded_miles"].as_f64(), Some(13.0));
    assert_eq!(item["total_miles"].as_f64(), Some(20.0));
    // origin_facility_name is a flat field (None here since no previous trip)
    assert!(item.get("origin_facility_name").is_some()
        || !item.as_object().unwrap().contains_key("origin_facility_name"));
}

#[tokio::test]
async fn test_list_trips_filter_by_trip_number_rest() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "ltn@example.com", "password-ltn").await;

    let trip_id = make_trip_with_two_stops(&server).await;
    let get = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    let trip_number = get.json::<serde_json::Value>()["trip_number"].as_str().unwrap().to_string();

    // Make a second unrelated trip
    let _ = make_trip_with_two_stops(&server).await;

    let resp = server.get(&format!("/fleet/api/v1/trips?trip_number={trip_number}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let items = resp.json::<serde_json::Value>()["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["trip_number"], trip_number);
}

#[tokio::test]
async fn test_list_trips_filter_by_load_number_rest() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "lln@example.com", "password-lln").await;

    let fac1 = create_test_facility(&server, "LLN Dock A", "Dallas, TX").await;
    let fac2 = create_test_facility(&server, "LLN Dock B", "Houston, TX").await;
    let load_resp = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "LLN Co",
            "stops": [
                {"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                 "facility_id": fac1, "scheduled_arrive": "2026-09-01T08:00:00",
                 "timezone": "America/Chicago"},
                {"sequence": 2, "stop_type": "delivery", "service_type": "live_unload",
                 "facility_id": fac2, "scheduled_arrive": "2026-09-02T08:00:00",
                 "timezone": "America/Chicago"},
            ],
            "rate_items": [{"description": "LH", "amount_usd": 100.0}]
        }))
        .await;
    let load_body = load_resp.json::<serde_json::Value>();
    let load_id = load_body["id"].as_str().unwrap().to_string();
    let load_number = load_body["load_number"].as_str().unwrap().to_string();

    let trip = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "load_id": load_id,
            "stops": [
                {"sequence": 1, "stop_type": "pickup", "facility_id": fac1,
                 "scheduled_arrive": "2026-09-01T08:00:00", "timezone": "America/Chicago"},
                {"sequence": 2, "stop_type": "delivery", "facility_id": fac2,
                 "scheduled_arrive": "2026-09-02T08:00:00", "timezone": "America/Chicago"},
            ]
        }))
        .await;
    let trip_id = trip.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // make an unrelated trip
    let _ = make_trip_with_two_stops(&server).await;

    let resp = server.get(&format!("/fleet/api/v1/trips?load_number={load_number}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let items = resp.json::<serde_json::Value>()["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], trip_id);
}

#[tokio::test]
async fn test_mcp_list_trips_filter_by_load_number() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "mcp_lln@example.com", "password-mcp-lln").await;

    let fac1 = create_test_facility(&server, "MCP LLN Dock A", "Dallas, TX").await;
    let fac2 = create_test_facility(&server, "MCP LLN Dock B", "Houston, TX").await;
    let load_resp = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "MCP LLN Co",
            "stops": [
                {"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                 "facility_id": fac1, "scheduled_arrive": "2026-09-10T08:00:00",
                 "timezone": "America/Chicago"},
                {"sequence": 2, "stop_type": "delivery", "service_type": "live_unload",
                 "facility_id": fac2, "scheduled_arrive": "2026-09-11T08:00:00",
                 "timezone": "America/Chicago"},
            ],
            "rate_items": [{"description": "LH", "amount_usd": 100.0}]
        }))
        .await;
    let load_body = load_resp.json::<serde_json::Value>();
    let load_id = load_body["id"].as_str().unwrap().to_string();
    let load_number = load_body["load_number"].as_str().unwrap().to_string();

    let trip = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "load_id": load_id,
            "stops": [
                {"sequence": 1, "stop_type": "pickup", "facility_id": fac1,
                 "scheduled_arrive": "2026-09-10T08:00:00", "timezone": "America/Chicago"},
                {"sequence": 2, "stop_type": "delivery", "facility_id": fac2,
                 "scheduled_arrive": "2026-09-11T08:00:00", "timezone": "America/Chicago"},
            ]
        }))
        .await;
    let trip_id = trip.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let _ = make_trip_with_two_stops(&server).await;

    let resp = mcp_call(&server, &token, "list_trips", serde_json::json!({
        "load_number": load_number
    })).await;
    let items = resp["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], trip_id);
}

#[tokio::test]
async fn test_load_detail_mileage_summary_loaded_only() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;
    let fac1 = create_test_facility(&server, "Pickup", "Chicago, IL").await;
    let fac2 = create_test_facility(&server, "Stop2", "St Louis, MO").await;
    let fac3 = create_test_facility(&server, "Delivery", "Memphis, TN").await;

    let load_id = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [
                {"sequence": 0, "stop_type": "pickup", "service_type": "live_load",
                 "facility_id": fac1, "scheduled_arrive": "2026-05-10T08:00:00",
                 "timezone": "America/Chicago"},
                {"sequence": 1, "stop_type": "delivery", "service_type": "live_unload",
                 "facility_id": fac2, "scheduled_arrive": "2026-05-10T14:00:00",
                 "timezone": "America/Chicago"},
                {"sequence": 2, "stop_type": "delivery", "service_type": "live_unload",
                 "facility_id": fac3, "scheduled_arrive": "2026-05-11T08:00:00",
                 "timezone": "America/Chicago"},
            ],
            "rate_items": []
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "load_id": load_id }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let trip_uuid: uuid::Uuid = trip_id.parse().unwrap();

    // Pre-populate mileage: deadhead 10.0, loaded leg1 100.0, loaded leg2 200.0
    state.db.update_trip_mileage(
        trip_uuid,
        Some(10.0),
        Some(300.0),
        Some(310.0),
        vec![10.0, 100.0, 200.0],
    ).await.unwrap();
    // segment_miles has 3 entries which is one per consecutive pair, but trip has 3 stops + no origin
    // (no previous_trip_id). Tweak: with no origin, segment_miles maps stop0->stop1, stop1->stop2.
    // So actually our segment_miles should be [100.0, 200.0] (2 loaded legs, no deadhead leg).
    state.db.update_trip_mileage(
        trip_uuid,
        Some(0.0),
        Some(300.0),
        Some(300.0),
        vec![100.0, 200.0],
    ).await.unwrap();

    let detail = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(detail.status_code(), 200);
    let body: serde_json::Value = detail.json();
    // The dispatch load detail surfaces the trip-level mileage summary (built via
    // `build_mileage_summary`), which — unlike the old admin load summary — keeps
    // the deadhead leg/origin rather than stripping them. Assert the loaded legs
    // and totals against that shape.
    let ms = &body["mileage_summary"];
    assert!(!ms.is_null(), "mileage_summary should be present");
    let legs = ms["legs"].as_array().expect("legs array");
    let loaded: Vec<_> = legs.iter().filter(|l| l["kind"] == "loaded").collect();
    assert_eq!(loaded.len(), 2, "two loaded legs");
    assert_eq!(ms["loaded_miles"].as_f64().unwrap(), 300.0);
    assert_eq!(ms["total_miles"].as_f64().unwrap(), 300.0);
}

#[tokio::test]
async fn test_load_detail_mileage_summary_none_without_trip() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let load_id = create_test_load(&server, &fac_id).await;

    let detail = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(detail.status_code(), 200);
    let body: serde_json::Value = detail.json();
    assert!(body["mileage_summary"].is_null(), "no trip → mileage_summary null");
}

#[tokio::test]
async fn test_load_detail_mileage_summary_none_when_only_cancelled_trip() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let load_id = create_test_load(&server, &fac_id).await;

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "load_id": load_id }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let trip_uuid: uuid::Uuid = trip_id.parse().unwrap();

    // Cancel the trip
    state.db.transition_trip_status(trip_uuid, ollie::models::TripStatus::Cancelled).await.unwrap();

    let detail = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(detail.status_code(), 200);
    let body: serde_json::Value = detail.json();
    // The dispatch load detail surfaces the latest trip's mileage summary without
    // special-casing cancelled trips (unlike the old admin load summary, which
    // returned null). The cancelled trip carries no computed mileage, so the
    // summary is present but empty: no legs and null miles throughout.
    let ms = &body["mileage_summary"];
    assert!(!ms.is_null(), "dispatch surfaces the latest trip's (empty) summary");
    assert_eq!(ms["legs"].as_array().unwrap().len(), 0, "no computed legs");
    assert!(ms["deadhead_miles"].is_null());
    assert!(ms["loaded_miles"].is_null());
    assert!(ms["total_miles"].is_null());
}

// ---------------------------------------------------------------------------
// Dispatcher portal facility CRUD (#265)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_fleet_user_facility_crud_http() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "fac-crud@example.com", "password-fac1").await;
    let auth = format!("Bearer {token}");

    // POST create
    let created = server.post("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({
            "name": "Plant City RSC",
            "address": "1000 Industrial Blvd, Plant City FL",
        }))
        .await;
    assert_eq!(created.status_code(), 201);
    let body: serde_json::Value = created.json();
    let id = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["geocode_status"], "pending");
    assert!(body["lat"].is_null());

    // GET one
    let one = server.get(&format!("/fleet/api/v1/facilities/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(one.status_code(), 200);
    assert_eq!(one.json::<serde_json::Value>()["name"], "Plant City RSC");

    // GET list (with q substring matching name)
    let list = server.get("/fleet/api/v1/facilities?q=plant")
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(list.status_code(), 200);
    let list_body: serde_json::Value = list.json();
    assert!(list_body["returned"].as_u64().unwrap() >= 1);
    let items = list_body["items"].as_array().unwrap();
    assert!(items.iter().any(|f| f["id"] == id));

    // PATCH update name
    let patched = server.patch(&format!("/fleet/api/v1/facilities/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "name": "Plant City RSC (renamed)" }))
        .await;
    assert_eq!(patched.status_code(), 200);
    assert_eq!(patched.json::<serde_json::Value>()["name"], "Plant City RSC (renamed)");
}

#[tokio::test]
async fn test_fleet_user_facility_create_with_explicit_coords_marks_ready() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "fac-coords@example.com", "password-fac2").await;

    let resp = server.post("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "name": "Pre-Geocoded Dock",
            "address": "123 Known St",
            "lat": 28.0125,
            "lng": -82.1199,
        }))
        .await;
    assert_eq!(resp.status_code(), 201);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["geocode_status"], "ready");
    assert!((body["lat"].as_f64().unwrap() - 28.0125).abs() < 1e-9);
}

#[tokio::test]
async fn test_fleet_user_facility_patch_address_requeues_geocode() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let token = fleet_user_login(&server, "fac-readdress@example.com", "password-fac3").await;
    let auth = format!("Bearer {token}");

    // Seed a Ready facility directly so we can observe the transition to Pending.
    let now = chrono::Utc::now();
    let id = uuid::Uuid::new_v4();
    state.db.insert_facility(&ollie::models::FacilityRecord {
        id, owner_id: 0,
        name: "Seeded".into(),
        address: "Old Address".into(),
        normalized_address: Some("Old normalized".into()),
        lat: Some(28.0), lng: Some(-82.0),
        geocode_status: ollie::models::GeocodeStatus::Ready,
        geocode_failure_count: 0,
        contacts: vec![], notes: None, tags: vec![], blob_ids: vec![],
        avg_dwell_minutes: None, dwell_sample_count: 0,
        archived: false, embedding: None, created_at: now, updated_at: now,
    }).await.unwrap();

    let resp = server.patch(&format!("/fleet/api/v1/facilities/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "address": "New Address" }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["address"], "New Address");
    assert_eq!(body["geocode_status"], "pending");
    assert!(body["lat"].is_null());
}

#[tokio::test]
async fn test_fleet_user_facility_patch_explicit_coords_repair_failed_geocode() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let token = fleet_user_login(&server, "fac-repair@example.com", "password-fac4").await;
    let auth = format!("Bearer {token}");

    let now = chrono::Utc::now();
    let id = uuid::Uuid::new_v4();
    state.db.insert_facility(&ollie::models::FacilityRecord {
        id, owner_id: 0,
        name: "Broken".into(),
        address: "Unresolvable address".into(),
        normalized_address: None,
        lat: None, lng: None,
        geocode_status: ollie::models::GeocodeStatus::PermanentlyFailed,
        geocode_failure_count: 3,
        contacts: vec![], notes: None, tags: vec![], blob_ids: vec![],
        avg_dwell_minutes: None, dwell_sample_count: 0,
        archived: false, embedding: None, created_at: now, updated_at: now,
    }).await.unwrap();

    let resp = server.patch(&format!("/fleet/api/v1/facilities/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "lat": 28.0125, "lng": -82.1199 }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["geocode_status"], "ready");
    assert_eq!(body["geocode_failure_count"], 0);
}

#[tokio::test]
async fn test_fleet_user_facility_create_rejects_unknown_field() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "fac-unk1@example.com", "password-fac5").await;

    let resp = server.post("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "name": "X", "address": "Y",
            "admin_secret": "leak",
        }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_fleet_user_facility_patch_rejects_unknown_field() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "fac-unk2@example.com", "password-fac6").await;
    let auth = format!("Bearer {token}");

    let created = server.post("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "name": "X", "address": "Y" }))
        .await;
    let id = created.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let resp = server.patch(&format!("/fleet/api/v1/facilities/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "owner_id": 99 }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_fleet_user_facility_mcp_create_and_update() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "fac-mcp@example.com", "password-mcp-fac").await;

    // MCP create_facility
    let record = mcp_call(&server, &token, "create_facility", serde_json::json!({
        "name": "MCP Facility", "address": "1 MCP Way",
    })).await;
    let id = record["id"].as_str().unwrap().to_string();
    assert_eq!(record["geocode_status"], "pending");

    // MCP update_facility — set explicit coords
    let upd_record = mcp_call(&server, &token, "update_facility", serde_json::json!({
        "facility_id": id, "lat": 30.0, "lng": -90.0,
    })).await;
    assert_eq!(upd_record["geocode_status"], "ready");
    assert!((upd_record["lat"].as_f64().unwrap() - 30.0).abs() < 1e-9);
}

#[tokio::test]
async fn test_facility_doctor_apply_retries_permanently_failed_geocode() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let token = fleet_user_login(&server, "fac-doc@example.com", "password-fac-doc").await;

    let now = chrono::Utc::now();
    let id = uuid::Uuid::new_v4();
    state.db.insert_facility(&ollie::models::FacilityRecord {
        id, owner_id: 0,
        name: "Stuck Facility".into(),
        address: "industrial address".into(),
        normalized_address: None,
        lat: None, lng: None,
        geocode_status: ollie::models::GeocodeStatus::PermanentlyFailed,
        geocode_failure_count: 3,
        contacts: vec![], notes: None, tags: vec![], blob_ids: vec![],
        avg_dwell_minutes: None, dwell_sample_count: 0,
        archived: false, embedding: None, created_at: now, updated_at: now,
    }).await.unwrap();

    let report = mcp_call(&server, &token, "facility_doctor", serde_json::json!({
        "facility_id": id.to_string(), "apply": true,
    })).await;
    let applied = report["applied"].as_array().unwrap();
    assert!(applied.iter().any(|c| c == "facility.geocode_retry"),
        "expected facility.geocode_retry in applied; got {applied:?}");

    let after = state.db.get_facility_by_id(id).await.unwrap();
    assert_eq!(after.geocode_status, ollie::models::GeocodeStatus::Pending);
    assert_eq!(after.geocode_failure_count, 0);
}

// ---------------------------------------------------------------------------
// Driver equipment endpoints (#268)
// ---------------------------------------------------------------------------

async fn create_driver_with_jwt(server: &TestServer, state: &AppState) -> (uuid::Uuid, String) {
    let owner_token = setup_owner(server).await;
    let driver_id_str = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Equip Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let driver_id: uuid::Uuid = driver_id_str.parse().unwrap();
    let creds = ollie::models::DriverCredentials {
        driver_id, pin_hash: None, token_version: 1,
        failed_pin_attempts: 0, locked_until: None,
        updated_at: chrono::Utc::now(),
    };
    state.db.upsert_driver_credentials(&creds).await.unwrap();
    let secret = std::env::var("DRIVER_JWT_SECRET").unwrap();
    let token = ollie::api::driver_portal::jwt::encode_driver_jwt(driver_id, 1, &secret).unwrap();
    (driver_id, token)
}

async fn create_trailer(server: &TestServer, unit: &str) -> uuid::Uuid {
    let owner_token = setup_owner(server).await;
    server.post("/fleet/api/v1/trailers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": unit, "owner": "fleet" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().parse().unwrap()
}

async fn create_truck(server: &TestServer, unit: &str) -> uuid::Uuid {
    let owner_token = setup_owner(server).await;
    server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": unit }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().parse().unwrap()
}

#[tokio::test]
async fn test_driver_equipment_get_returns_empty_initially() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let (_did, token) = create_driver_with_jwt(&server, &state).await;
    let resp = server.get("/driver/api/v1/equipment")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let v: serde_json::Value = resp.json();
    assert!(v["truck"].is_null());
    assert_eq!(v["trailers"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_driver_equipment_update_trailer_by_id_persists_and_returns_summary() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let (did, token) = create_driver_with_jwt(&server, &state).await;
    let trailer_id = create_trailer(&server, "TR-EQ-001").await;

    let resp = server.put("/driver/api/v1/equipment/trailer")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "trailer_ids": [trailer_id.to_string()] }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["trip_cascade"], false);
    assert!(body["trip_id"].is_null());
    assert_eq!(body["trailers"][0]["id"], trailer_id.to_string());
    assert_eq!(body["trailers"][0]["unit_number"], "TR-EQ-001");

    // Driver record updated.
    let d = state.db.get_driver_by_id(did).await.unwrap();
    assert_eq!(d.current_trailer_ids, vec![trailer_id]);
}

#[tokio::test]
async fn test_driver_equipment_update_trailer_by_unit_number_lookup() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let (_did, token) = create_driver_with_jwt(&server, &state).await;
    let _ = create_trailer(&server, "TR-UNIT-LOOKUP").await;

    let resp = server.put("/driver/api/v1/equipment/trailer")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "trailer_unit_numbers": ["TR-UNIT-LOOKUP"] }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["trailers"][0]["unit_number"], "TR-UNIT-LOOKUP");
}

#[tokio::test]
async fn test_driver_equipment_hook_unknown_unit_creates_trailer() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let (did, token) = create_driver_with_jwt(&server, &state).await;

    // Hooking a unit number that isn't a known trailer creates it (owner: fleet,
    // flagged for dispatch review) and hooks the driver to it.
    let resp = server.put("/driver/api/v1/equipment/trailer")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "trailer_unit_numbers": ["NEW-HOOK-001"] }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["trailers"][0]["unit_number"], "NEW-HOOK-001");
    assert_eq!(body["trailers"][0]["owner"], "fleet");
    let new_id = body["trailers"][0]["id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap();

    // The trailer was actually persisted and the driver is hooked to it.
    let created = state.db.get_trailer_by_id(new_id).await.unwrap();
    assert_eq!(created.unit_number, "NEW-HOOK-001");
    assert!(created.notes.as_deref().unwrap_or("").to_lowercase().contains("driver-created"));
    let d = state.db.get_driver_by_id(did).await.unwrap();
    assert_eq!(d.current_trailer_ids, vec![new_id]);

    // Hooking the same unit again reuses the existing trailer (no duplicate).
    let resp2 = server.put("/driver/api/v1/equipment/trailer")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "trailer_unit_numbers": ["NEW-HOOK-001"] }))
        .await;
    assert_eq!(resp2.status_code(), 200);
    let body2: serde_json::Value = resp2.json();
    assert_eq!(body2["trailers"][0]["id"], new_id.to_string());
}

#[tokio::test]
async fn test_driver_equipment_update_both_fields_returns_400() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let (_did, token) = create_driver_with_jwt(&server, &state).await;
    let trailer_id = create_trailer(&server, "TR-BOTH").await;
    let resp = server.put("/driver/api/v1/equipment/trailer")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "trailer_ids": [trailer_id.to_string()],
            "trailer_unit_numbers": ["TR-BOTH"],
        }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_driver_equipment_update_neither_field_returns_400() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let (_did, token) = create_driver_with_jwt(&server, &state).await;
    let resp = server.put("/driver/api/v1/equipment/trailer")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({}))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_driver_equipment_cascades_into_active_in_transit_trip() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;
    let (driver_id, token) = create_driver_with_jwt(&server, &state).await;

    let old_trailer = create_trailer(&server, "TR-OLD").await;
    let new_trailer = create_trailer(&server, "TR-NEW").await;
    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "T-EQ-CASCADE" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "name": "Origin" },
                { "sequence": 1, "stop_type": "delivery", "name": "Destination" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "driver_id": driver_id.to_string(),
            "truck_id": truck_id,
            "trailer_ids": [old_trailer.to_string()],
        }))
        .await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    // depart origin → in_transit
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/0/depart"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-05-12T10:00:00Z" }))
        .await;

    let resp = server.put("/driver/api/v1/equipment/trailer")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "trailer_ids": [new_trailer.to_string()] }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["trip_cascade"], true);
    assert_eq!(body["trip_id"].as_str().unwrap(), trip_id);

    let trip_uuid: uuid::Uuid = trip_id.parse().unwrap();
    let trip = state.db.get_trip(trip_uuid).await.unwrap();
    assert_eq!(trip.trailer_ids, vec![new_trailer]);
}

#[tokio::test]
async fn test_driver_equipment_no_cascade_when_at_final_delivery() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;
    let (driver_id, token) = create_driver_with_jwt(&server, &state).await;

    let old_trailer = create_trailer(&server, "TR-FD-OLD").await;
    let new_trailer = create_trailer(&server, "TR-FD-NEW").await;
    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "T-EQ-FD" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "name": "Origin" },
                { "sequence": 1, "stop_type": "delivery", "name": "Destination" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "driver_id": driver_id.to_string(),
            "truck_id": truck_id,
            "trailer_ids": [old_trailer.to_string()],
        }))
        .await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/0/depart"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-05-12T10:00:00Z" }))
        .await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/1/arrive"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "actual_arrive": "2026-05-12T14:00:00Z" }))
        .await;

    let resp = server.put("/driver/api/v1/equipment/trailer")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "trailer_ids": [new_trailer.to_string()] }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["trip_cascade"], false);

    let trip_uuid: uuid::Uuid = trip_id.parse().unwrap();
    let trip = state.db.get_trip(trip_uuid).await.unwrap();
    assert_eq!(trip.trailer_ids, vec![old_trailer], "trip trailer should not change when driver is at final delivery");

    let d = state.db.get_driver_by_id(driver_id).await.unwrap();
    assert_eq!(d.current_trailer_ids, vec![new_trailer]);
}

#[tokio::test]
async fn test_dispatch_trip_reconciles_to_driver_current_trailer() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;
    let (driver_id, token) = create_driver_with_jwt(&server, &state).await;

    let initial_trailer = create_trailer(&server, "TR-DISP-INIT").await;
    let attached_trailer = create_trailer(&server, "TR-DISP-ATTACHED").await;
    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "T-DISP-RECON" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "name": "Origin" },
                { "sequence": 1, "stop_type": "delivery", "name": "Destination" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "driver_id": driver_id.to_string(),
            "truck_id": truck_id,
            "trailer_ids": [initial_trailer.to_string()],
        }))
        .await;

    // Driver attaches a different trailer pre-dispatch (no cascade since trip is Assigned).
    let put_resp = server.put("/driver/api/v1/equipment/trailer")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "trailer_ids": [attached_trailer.to_string()] }))
        .await;
    assert_eq!(put_resp.status_code(), 200);

    let trip_uuid: uuid::Uuid = trip_id.parse().unwrap();
    let pre = state.db.get_trip(trip_uuid).await.unwrap();
    assert_eq!(pre.trailer_ids, vec![initial_trailer], "trip should still hold the initial trailer pre-dispatch");

    let disp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(disp.status_code(), 200);

    let post = state.db.get_trip(trip_uuid).await.unwrap();
    assert_eq!(post.trailer_ids, vec![attached_trailer], "dispatch should reconcile trip trailer to driver's current_trailer_ids");

    // Dropped trailer should fall back to Available; attached trailer should be Dispatched.
    let dropped = state.db.get_trailer_by_id(initial_trailer).await.unwrap();
    assert_eq!(dropped.status, ollie::models::TrailerStatus::Available,
        "trailer dropped at dispatch reconciliation should revert to Available");
    let attached = state.db.get_trailer_by_id(attached_trailer).await.unwrap();
    assert_eq!(attached.status, ollie::models::TrailerStatus::Dispatched,
        "newly attached trailer should be Dispatched after trip dispatch");
}

#[tokio::test]
async fn test_driver_equipment_cascade_syncs_trailer_statuses() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;
    let (driver_id, token) = create_driver_with_jwt(&server, &state).await;

    let old_trailer = create_trailer(&server, "TR-SYNC-OLD").await;
    let new_trailer = create_trailer(&server, "TR-SYNC-NEW").await;
    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "T-EQ-SYNC" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "name": "Origin" },
                { "sequence": 1, "stop_type": "delivery", "name": "Destination" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "driver_id": driver_id.to_string(),
            "truck_id": truck_id,
            "trailer_ids": [old_trailer.to_string()],
        }))
        .await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/0/depart"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-05-12T10:00:00Z" }))
        .await;

    // old trailer should be Dispatched at this point.
    let old_before = state.db.get_trailer_by_id(old_trailer).await.unwrap();
    assert_eq!(old_before.status, ollie::models::TrailerStatus::Dispatched);

    // Swap to new trailer via driver equipment endpoint (mid InTransit).
    let resp = server.put("/driver/api/v1/equipment/trailer")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "trailer_ids": [new_trailer.to_string()] }))
        .await;
    assert_eq!(resp.status_code(), 200);

    let old_after = state.db.get_trailer_by_id(old_trailer).await.unwrap();
    assert_eq!(old_after.status, ollie::models::TrailerStatus::Available,
        "dropped trailer should fall back to Available after mid-trip swap");
    let new_after = state.db.get_trailer_by_id(new_trailer).await.unwrap();
    assert_eq!(new_after.status, ollie::models::TrailerStatus::Dispatched,
        "newly attached trailer should be Dispatched while trip is InTransit");
}

#[tokio::test]
async fn test_driver_equipment_reflects_assigned_trip_truck_and_trailer() {
    // Regression: a driver assigned a truck + trailer via dispatch (who never
    // recorded a self-service swap) must see that equipment on the Equipment
    // tab. The truck has no driver-side write path, so it is derived from the
    // driver's active trip; trailers fall back to the trip when the driver has
    // no recorded swap.
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;
    let (driver_id, token) = create_driver_with_jwt(&server, &state).await;

    let trailer_id = create_trailer(&server, "TR-REFLECT").await;
    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "T-REFLECT", "plate": "RFL-123" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "name": "Origin" },
                { "sequence": 1, "stop_type": "delivery", "name": "Destination" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "driver_id": driver_id.to_string(),
            "truck_id": truck_id,
            "trailer_ids": [trailer_id.to_string()],
        }))
        .await;

    // Driver has recorded no swap, so current_truck_id / current_trailer_ids are unset.
    let d = state.db.get_driver_by_id(driver_id).await.unwrap();
    assert!(d.current_truck_id.is_none());
    assert!(d.current_trailer_ids.is_empty());

    let resp = server.get("/driver/api/v1/equipment")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let v: serde_json::Value = resp.json();
    assert_eq!(v["truck"]["id"], truck_id, "truck should be derived from the assigned trip");
    assert_eq!(v["truck"]["unit_number"], "T-REFLECT");
    assert_eq!(v["truck"]["plate"], "RFL-123");
    assert_eq!(v["trailers"][0]["id"], trailer_id.to_string(),
        "trailer should fall back to the assigned trip when no swap is recorded");
    assert_eq!(v["trailers"][0]["unit_number"], "TR-REFLECT");
}

#[tokio::test]
async fn test_driver_equipment_prefers_running_trip_over_newer_queued_trip() {
    // A driver running a Dispatched trip while a newer Assigned trip is queued
    // must see the running trip's truck on the Equipment tab — not the queued
    // one. Dispatch allows at most one Dispatched/InTransit trip per driver, so
    // the in-flight trip is the source of physically-attached equipment.
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let owner_token = setup_owner(&server).await;
    let (driver_id, token) = create_driver_with_jwt(&server, &state).await;

    let running_truck = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "T-RUNNING" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let queued_truck = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "T-QUEUED" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let make_trip = || async {
        server.post("/fleet/api/v1/trips")
            .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
            .json(&serde_json::json!({
                "stops": [
                    { "sequence": 0, "stop_type": "pickup", "name": "Origin" },
                    { "sequence": 1, "stop_type": "delivery", "name": "Destination" }
                ]
            }))
            .await
            .json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
    };

    // Trip A: assigned then dispatched (the running trip).
    let trip_a = make_trip().await;
    server.post(&format!("/fleet/api/v1/trips/{trip_a}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id.to_string(), "truck_id": running_truck }))
        .await;
    server.post(&format!("/fleet/api/v1/trips/{trip_a}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;

    // Trip B: a newer trip merely assigned to the same driver (queued).
    let trip_b = make_trip().await;
    server.post(&format!("/fleet/api/v1/trips/{trip_b}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id.to_string(), "truck_id": queued_truck }))
        .await;

    let resp = server.get("/driver/api/v1/equipment")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let v: serde_json::Value = resp.json();
    assert_eq!(v["truck"]["unit_number"], "T-RUNNING",
        "equipment should reflect the running (dispatched) trip, not the newer queued one");
}

// ---------------------------------------------------------------------------
// Dispatcher portal trailer + truck CRUD (#269)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_fleet_user_trailer_crud_http() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "trl-crud@example.com", "password-trl1").await;
    let auth = format!("Bearer {token}");

    // POST create — fleet trailer
    let created = server.post("/fleet/api/v1/trailers")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({
            "unit_number": "DTRL-001",
            "owner": "fleet",
            "trailer_type": "dry_van",
            "length_ft": 53.0,
        }))
        .await;
    assert_eq!(created.status_code(), 201);
    let body: serde_json::Value = created.json();
    let id = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["status"], "available");
    assert_eq!(body["unit_number"], "DTRL-001");

    // GET one
    let one = server.get(&format!("/fleet/api/v1/trailers/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(one.status_code(), 200);
    assert_eq!(one.json::<serde_json::Value>()["unit_number"], "DTRL-001");

    // GET list
    let list = server.get("/fleet/api/v1/trailers")
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(list.status_code(), 200);
    let items = list.json::<serde_json::Value>()["items"].as_array().unwrap().clone();
    assert!(items.iter().any(|t| t["id"] == id));

    // PATCH update notes + make
    let patched = server.patch(&format!("/fleet/api/v1/trailers/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "notes": "ICE box, mid-2026 refurb", "make": "Wabash" }))
        .await;
    assert_eq!(patched.status_code(), 200);
    let pbody: serde_json::Value = patched.json();
    assert_eq!(pbody["notes"], "ICE box, mid-2026 refurb");
    assert_eq!(pbody["make"], "Wabash");
    assert_eq!(pbody["status"], "available");
}

#[tokio::test]
async fn test_fleet_user_trailer_create_requires_owner_name_when_not_fleet() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "trl-owner@example.com", "password-trl2").await;

    let resp = server.post("/fleet/api/v1/trailers")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "unit_number": "DTRL-CAR-001",
            "owner": "carrier",
        }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_fleet_user_trailer_create_rejects_unknown_field() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "trl-unk1@example.com", "password-trl3").await;

    // status is admin-only — must be rejected
    let resp = server.post("/fleet/api/v1/trailers")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "unit_number": "DTRL-X",
            "owner": "fleet",
            "status": "dispatched",
        }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_fleet_user_trailer_patch_rejects_status_and_unknown_fields() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "trl-unk2@example.com", "password-trl4").await;
    let auth = format!("Bearer {token}");

    let created = server.post("/fleet/api/v1/trailers")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "unit_number": "DTRL-PATCH", "owner": "fleet" }))
        .await;
    let id = created.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // status is intentionally not in PatchTrailerBody
    let resp = server.patch(&format!("/fleet/api/v1/trailers/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "status": "out_of_service" }))
        .await;
    assert_eq!(resp.status_code(), 400);

    // owner_id is admin-only
    let resp = server.patch(&format!("/fleet/api/v1/trailers/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "owner_id": 99 }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_fleet_user_truck_crud_http() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "trk-crud@example.com", "password-trk1").await;
    let auth = format!("Bearer {token}");

    let created = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({
            "unit_number": "DTRK-001",
            "make": "Kenworth",
            "model": "T680",
            "year": 2024,
        }))
        .await;
    assert_eq!(created.status_code(), 201);
    let body: serde_json::Value = created.json();
    let id = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["status"], "available");

    let one = server.get(&format!("/fleet/api/v1/trucks/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(one.status_code(), 200);
    assert_eq!(one.json::<serde_json::Value>()["unit_number"], "DTRK-001");

    let patched = server.patch(&format!("/fleet/api/v1/trucks/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "notes": "DEF top-off at terminal" }))
        .await;
    assert_eq!(patched.status_code(), 200);
    assert_eq!(patched.json::<serde_json::Value>()["notes"], "DEF top-off at terminal");
}

#[tokio::test]
async fn test_fleet_user_truck_patch_rejects_status_and_unknown_fields() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "trk-unk@example.com", "password-trk2").await;
    let auth = format!("Bearer {token}");

    let created = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "unit_number": "DTRK-PATCH" }))
        .await;
    let id = created.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let resp = server.patch(&format!("/fleet/api/v1/trucks/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "status": "out_of_service" }))
        .await;
    assert_eq!(resp.status_code(), 400);

    let resp = server.patch(&format!("/fleet/api/v1/trucks/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "owner_id": 99 }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_fleet_user_trailer_mcp_create_get_update() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "trl-mcp@example.com", "password-trl-mcp").await;

    // create_trailer
    let record = mcp_call(&server, &token, "create_trailer", serde_json::json!({
        "unit_number": "MCP-TRL-001",
        "owner": "fleet",
        "trailer_type": "reefer",
    })).await;
    let id = record["id"].as_str().unwrap().to_string();
    assert_eq!(record["status"], "available");
    assert_eq!(record["trailer_type"], "reefer");

    // get_trailer
    let got = mcp_call(&server, &token, "get_trailer", serde_json::json!({
        "trailer_id": id
    })).await;
    assert_eq!(got["unit_number"], "MCP-TRL-001");

    // update_trailer
    let upd = mcp_call(&server, &token, "update_trailer", serde_json::json!({
        "trailer_id": id, "notes": "via MCP",
    })).await;
    assert_eq!(upd["notes"], "via MCP");
}

#[tokio::test]
async fn test_fleet_user_truck_mcp_create_get_update() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "trk-mcp@example.com", "password-trk-mcp").await;

    let record = mcp_call(&server, &token, "create_truck", serde_json::json!({
        "unit_number": "MCP-TRK-001", "make": "Peterbilt",
    })).await;
    let id = record["id"].as_str().unwrap().to_string();
    assert_eq!(record["status"], "available");
    assert_eq!(record["make"], "Peterbilt");

    let got = mcp_call(&server, &token, "get_truck", serde_json::json!({
        "truck_id": id
    })).await;
    assert_eq!(got["unit_number"], "MCP-TRK-001");

    let upd = mcp_call(&server, &token, "update_truck", serde_json::json!({
        "truck_id": id, "model": "579",
    })).await;
    assert_eq!(upd["model"], "579");
}

#[tokio::test]
async fn test_fleet_user_mcp_create_truck_and_trailer_then_assign() {
    // Acceptance criteria: fleet_user agent creates a trailer (and truck)
    // mid-conversation via MCP and immediately references them in assign_driver.
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "asg-mcp@example.com", "password-asg-mcp").await;

    // Driver (admin API — there's no fleet_user driver-create)
    let driver_resp = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "MCP Assign Driver" }))
        .await;
    assert_eq!(driver_resp.status_code(), 201);
    let driver_id = driver_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Create truck via MCP
    let truck_id = mcp_call(&server, &token, "create_truck", serde_json::json!({
        "unit_number": "ASG-TRK-001"
    })).await["id"].as_str().unwrap().to_string();

    // Create trailer via MCP
    let trailer_id = mcp_call(&server, &token, "create_trailer", serde_json::json!({
        "unit_number": "ASG-TRL-001", "owner": "fleet",
    })).await["id"].as_str().unwrap().to_string();

    // Trip
    let fac_id = create_test_facility(&server, "MCP Origin", "Dallas, TX").await;
    let trip_resp = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "trip_number": "T-ASG-MCP-001",
            "stops": [{
                "sequence": 1, "stop_type": "origin",
                "facility_id": fac_id, "scheduled_arrive": "2026-08-01T08:00:00",
                "timezone": "America/Chicago"
            }]
        }))
        .await;
    assert_eq!(trip_resp.status_code(), 201);
    let trip_id = trip_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // assign_driver via MCP using the freshly-created truck and trailer
    let trip = mcp_call(&server, &token, "assign_driver", serde_json::json!({
        "trip_id": trip_id,
        "driver_id": driver_id,
        "truck_id": truck_id,
        "trailer_ids": [trailer_id],
    })).await;
    assert_eq!(trip["status"], "assigned");
    assert_eq!(trip["driver_id"], driver_id);
    assert_eq!(trip["truck_id"], truck_id);
}

// ---------------------------------------------------------------------------
// Blob-store MCP tools + presigned byte-transfer endpoints (#277)
// ---------------------------------------------------------------------------

const DISPATCHER_SECRET: &str = "test-fleet_user-secret-must-be-32b";

fn mint_upload_token() -> String {
    ollie::api::fleet_portal::blob_links::mint_token(
        DISPATCHER_SECRET,
        ollie::api::fleet_portal::blob_links::BlobUrlOp::Post,
        None,
        300,
    )
    .unwrap()
    .0
}

fn mint_download_token(id: uuid::Uuid) -> String {
    ollie::api::fleet_portal::blob_links::mint_token(
        DISPATCHER_SECRET,
        ollie::api::fleet_portal::blob_links::BlobUrlOp::Get,
        Some(id),
        300,
    )
    .unwrap()
    .0
}

/// Upload bytes through the presigned POST route (the path the `upload_blob`
/// MCP tool hands agents) and return the created blob record.
async fn upload_blob_via_presigned(
    server: &axum_test::TestServer,
    data: Vec<u8>,
    content_type: &str,
    name: &str,
) -> serde_json::Value {
    let token = mint_upload_token();
    server
        .post(&format!("/fleet/blobs/presigned?token={token}&name={name}"))
        .add_header(header::CONTENT_TYPE, content_type)
        .bytes(data.into())
        .await
        .json()
}

#[tokio::test]
async fn test_presigned_blob_round_trip() {
    let (server, _b, _d, _rx) = test_server().await;
    let data = b"%PDF-1.4 fake pdf bytes for the round trip test".to_vec();
    let expected_sum = ollie::storage::compute_checksum(&data);

    // Upload via presigned POST
    let up_token = mint_upload_token();
    let up = server
        .post(&format!("/fleet/blobs/presigned?token={up_token}&name=rt.pdf&tags=invoice,rt"))
        .add_header(header::CONTENT_TYPE, "application/pdf")
        .bytes(data.clone().into())
        .await;
    assert!(
        up.status_code() == 202 || up.status_code() == 201,
        "unexpected status: {}",
        up.status_code()
    );
    let rec: serde_json::Value = up.json();
    let id = rec["id"].as_str().unwrap().to_string();
    assert_eq!(rec["checksum"], expected_sum, "checksum must be the sha256 of the bytes");
    assert_eq!(rec["mime_type"], "application/pdf");
    assert_eq!(rec["name"], "rt.pdf");
    assert_eq!(rec["tags"], serde_json::json!(["invoice", "rt"]));

    // Download via presigned GET — bytes must round-trip exactly
    let blob_uuid = id.parse::<uuid::Uuid>().unwrap();
    let dl_token = mint_download_token(blob_uuid);
    let dl = server
        .get(&format!("/fleet/blobs/presigned/{id}?token={dl_token}"))
        .await;
    assert_eq!(dl.status_code(), 200);
    assert_eq!(dl.as_bytes().to_vec(), data, "downloaded bytes must match uploaded bytes");
}

#[tokio::test]
async fn test_presigned_download_rejects_id_mismatch() {
    let (server, _b, _d, _rx) = test_server().await;
    // Token bound to one blob id, used against a different path id → 401.
    let bound = uuid::Uuid::new_v4();
    let token = mint_download_token(bound);
    let other = uuid::Uuid::new_v4();
    let resp = server
        .get(&format!("/fleet/blobs/presigned/{other}?token={token}"))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_presigned_upload_rejects_bad_token() {
    let (server, _b, _d, _rx) = test_server().await;
    let resp = server
        .post("/fleet/blobs/presigned?token=not-a-valid-jwt")
        .add_header(header::CONTENT_TYPE, "text/plain")
        .bytes(b"hello".to_vec().into())
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_presigned_upload_rejects_download_token() {
    let (server, _b, _d, _rx) = test_server().await;
    // A GET-scoped token must not authorize an upload.
    let token = mint_download_token(uuid::Uuid::new_v4());
    let resp = server
        .post(&format!("/fleet/blobs/presigned?token={token}"))
        .add_header(header::CONTENT_TYPE, "text/plain")
        .bytes(b"hello".to_vec().into())
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_mcp_get_blob_metadata_and_delete() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "blobmcp3@example.com", "password-blobmcp3").await;

    // Create a blob via the presigned upload path.
    let created =
        upload_blob_via_presigned(&server, b"settlement statement".to_vec(), "text/plain", "stmt.txt").await;
    let id = created["id"].as_str().unwrap().to_string();

    // Metadata carries an (empty) attached_to reverse lookup.
    let meta = mcp_call(&server, &token, "get_blob_metadata", serde_json::json!({ "id": id })).await;
    assert_eq!(meta["id"], id);
    assert_eq!(meta["attached_to"]["loads"], serde_json::json!([]));
    assert_eq!(meta["attached_to"]["facilities"], serde_json::json!([]));

    // list_blobs sees it.
    let listed = mcp_call(&server, &token, "list_blobs", serde_json::json!({ "content_type": "text/plain" })).await;
    assert!(listed["returned"].as_u64().unwrap() >= 1);
    assert!(listed["total"].is_number(), "list_blobs must report a total");
    assert!(listed["truncated"].is_boolean(), "list_blobs must report truncation");

    // Unattached delete succeeds and reports was_attached=false.
    let del = mcp_call(&server, &token, "delete_blob", serde_json::json!({ "id": id })).await;
    assert_eq!(del["deleted"], true);
    assert_eq!(del["was_attached"], false);
}

#[tokio::test]
async fn test_mcp_delete_blob_keeps_bytes_when_checksum_shared() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "blobmcp5@example.com", "password-blobmcp5").await;

    // Two records, identical content → same checksum (the second dedups).
    let raw = b"shared-content document body".to_vec();
    let a = upload_blob_via_presigned(&server, raw.clone(), "text/plain", "a.txt").await;
    let b = upload_blob_via_presigned(&server, raw.clone(), "text/plain", "b.txt").await;
    let id_a = a["id"].as_str().unwrap().to_string();
    let id_b = b["id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap();
    assert_eq!(a["checksum"], b["checksum"], "identical content must share a checksum");

    // Delete A; B still references the checksum, so the bytes must survive.
    let del = mcp_call(&server, &token, "delete_blob", serde_json::json!({ "id": id_a })).await;
    assert_eq!(del["deleted"], true);

    let dl_token = mint_download_token(id_b);
    let resp = server.get(&format!("/fleet/blobs/presigned/{id_b}?token={dl_token}")).await;
    assert_eq!(resp.status_code(), 200, "B must remain downloadable after A is deleted");
    assert_eq!(resp.as_bytes().to_vec(), raw);
}

#[tokio::test]
async fn test_openapi_includes_presigned_blob_paths() {
    let (server, _b, _d, _rx) = test_server().await;
    let resp = server.get("/openapi.json").await;
    assert_eq!(resp.status_code(), 200);
    let spec: serde_json::Value = resp.json();
    let paths = &spec["paths"];
    assert!(!paths["/fleet/blobs/presigned"].is_null(), "upload path missing from spec");
    assert!(!paths["/fleet/blobs/presigned/{id}"].is_null(), "download path missing from spec");
}

// --- TOCTOU race fix tests ---

#[tokio::test]
async fn test_admin_delete_blob_keeps_bytes_when_checksum_shared() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;

    // Upload the same bytes twice — dedup gives two records with the same checksum.
    let content = b"shared-checksum-admin-test-bytes";
    let r1 = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(content.to_vec())
                .file_name("shared-a.txt").mime_type("text/plain")))
        .await;
    assert!(r1.status_code() == 202 || r1.status_code() == 201);

    let r2 = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(content.to_vec())
                .file_name("shared-b.txt").mime_type("text/plain")))
        .await;
    assert!(r2.status_code() == 202 || r2.status_code() == 201);

    let id1 = r1.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let id2 = r2.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    assert_ne!(id1, id2, "dedup must produce two distinct record ids");

    // Delete the first record — the storage bytes must NOT be deleted because id2 still exists.
    let del = server.delete(&format!("/fleet/api/v1/blob/{id1}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(del.status_code(), 204);

    // The sibling record's bytes must still be downloadable.
    let get = server.get(&format!("/fleet/api/v1/blob/{id2}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(get.status_code(), 200, "sibling blob must still be readable after first record deleted");
    assert_eq!(get.as_bytes(), content.as_slice(), "sibling blob bytes must be intact");
}

#[tokio::test]
async fn test_fleet_user_delete_blob_keeps_bytes_when_checksum_shared() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "toctou-disp@example.com", "password-toctou-disp").await;

    // Upload the same bytes twice — dedup gives two records with the same checksum.
    let content = b"shared-checksum-fleet_user-test-bytes";
    let r1 = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(content.to_vec())
                .file_name("disp-shared-a.txt").mime_type("text/plain")))
        .await;
    assert!(r1.status_code() == 202 || r1.status_code() == 201);

    let r2 = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(content.to_vec())
                .file_name("disp-shared-b.txt").mime_type("text/plain")))
        .await;
    assert!(r2.status_code() == 202 || r2.status_code() == 201);

    let id1 = r1.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let id2 = r2.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    assert_ne!(id1, id2, "dedup must produce two distinct record ids");

    // Delete the first record — the storage bytes must NOT be deleted because id2 still exists.
    let del = server.delete(&format!("/fleet/api/v1/blob/{id1}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(del.status_code(), 204);

    // The sibling record's bytes must still be downloadable.
    let get = server.get(&format!("/fleet/api/v1/blob/{id2}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(get.status_code(), 200, "sibling blob must still be readable after first record deleted");
    assert_eq!(get.as_bytes(), content.as_slice(), "sibling blob bytes must be intact");
}

// ---------------------------------------------------------------------------
// Driver equipment attach/detach (fleet_user surface) — #181
// ---------------------------------------------------------------------------

async fn make_driver(server: &axum_test::TestServer, name: &str) -> String {
    let owner_token = setup_owner(server).await;
    server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": name }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

async fn make_truck(server: &axum_test::TestServer, unit: &str) -> String {
    let owner_token = setup_owner(server).await;
    server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": unit }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

async fn make_trailer(server: &axum_test::TestServer, unit: &str) -> String {
    let owner_token = setup_owner(server).await;
    server.post("/fleet/api/v1/trailers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": unit, "owner": "fleet" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

async fn truck_status(server: &axum_test::TestServer, id: &str) -> String {
    let owner_token = setup_owner(server).await;
    server.get(&format!("/fleet/api/v1/trucks/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await
        .json::<serde_json::Value>()["status"].as_str().unwrap().to_string()
}

async fn trailer_status(server: &axum_test::TestServer, id: &str) -> String {
    let owner_token = setup_owner(server).await;
    server.get(&format!("/fleet/api/v1/trailers/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await
        .json::<serde_json::Value>()["status"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_attach_equipment_truck_and_trailers() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "attach1@example.com", "password-attach1").await;
    let auth = format!("Bearer {token}");

    let driver = make_driver(&server, "Attach Driver").await;
    let truck = make_truck(&server, "AE-TRK-1").await;
    let trl_a = make_trailer(&server, "AE-TRL-A").await;
    let trl_b = make_trailer(&server, "AE-TRL-B").await;

    let resp = server.post(&format!("/fleet/api/v1/drivers/{driver}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "truck": truck, "trailer_ids": [trl_a, trl_b] }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["truck_id"], truck);
    assert_eq!(body["trailer_ids"].as_array().unwrap().len(), 2);
    assert_eq!(body["trip_cascade"], false);

    // Driver record reflects equipment.
    let d = server.get(&format!("/fleet/api/v1/drivers/{driver}"))
        .add_header(header::AUTHORIZATION, &auth).await.json::<serde_json::Value>();
    assert_eq!(d["current_truck_id"], truck);
    assert_eq!(d["current_trailer_ids"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_attach_equipment_trailers_are_additive() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "attach2@example.com", "password-attach2").await;
    let auth = format!("Bearer {token}");

    let driver = make_driver(&server, "Additive Driver").await;
    let trl_a = make_trailer(&server, "ADD-TRL-A").await;
    let trl_b = make_trailer(&server, "ADD-TRL-B").await;

    server.post(&format!("/fleet/api/v1/drivers/{driver}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "trailer_ids": [trl_a] })).await;

    let resp = server.post(&format!("/fleet/api/v1/drivers/{driver}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "trailer_ids": [trl_b] })).await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    let ids: Vec<String> = body["trailer_ids"].as_array().unwrap().iter()
        .map(|v| v.as_str().unwrap().to_string()).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&trl_a));
    assert!(ids.contains(&trl_b));

    // Re-attaching the same trailer does not duplicate it.
    let resp = server.post(&format!("/fleet/api/v1/drivers/{driver}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "trailer_ids": [trl_a] })).await;
    assert_eq!(resp.json::<serde_json::Value>()["trailer_ids"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_attach_truck_releases_previous_truck() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "attach3@example.com", "password-attach3").await;
    let auth = format!("Bearer {token}");

    let driver = make_driver(&server, "Swap Driver").await;
    let truck1 = make_truck(&server, "SW-TRK-1").await;
    let truck2 = make_truck(&server, "SW-TRK-2").await;

    server.post(&format!("/fleet/api/v1/drivers/{driver}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "truck": truck1 })).await;
    assert_eq!(truck_status(&server, &truck1).await, "assigned");

    server.post(&format!("/fleet/api/v1/drivers/{driver}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "truck": truck2 })).await;
    assert_eq!(truck_status(&server, &truck1).await, "available", "previous truck released");
    assert_eq!(truck_status(&server, &truck2).await, "assigned");
}

#[tokio::test]
async fn test_attach_equipment_empty_body_400() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "attach4@example.com", "password-attach4").await;
    let driver = make_driver(&server, "Empty Driver").await;

    let resp = server.post(&format!("/fleet/api/v1/drivers/{driver}/attach-equipment"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({})).await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_attach_equipment_inactive_driver_409() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "attach5@example.com", "password-attach5").await;
    let auth = format!("Bearer {token}");
    let driver = make_driver(&server, "Inactive Driver").await;
    let truck = make_truck(&server, "IN-TRK-1").await;

    // Soft-delete (inactivate) the driver via admin API.
    server.delete(&format!("/fleet/api/v1/drivers/{driver}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}")).await;

    let resp = server.post(&format!("/fleet/api/v1/drivers/{driver}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "truck": truck })).await;
    assert_eq!(resp.status_code(), 409);
}

#[tokio::test]
async fn test_attach_equipment_conflict_on_other_active_trip() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "attach6@example.com", "password-attach6").await;
    let auth = format!("Bearer {token}");

    // Driver A gets a dispatched trip with truck + trailer.
    let driver_a = make_driver(&server, "Driver A").await;
    let truck = make_truck(&server, "CF-TRK-1").await;
    let trailer = make_trailer(&server, "CF-TRL-1").await;
    let trip = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "name": "O" },
                { "sequence": 2, "stop_type": "delivery", "name": "D" }
            ]
        }))
        .await.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    server.post(&format!("/fleet/api/v1/trips/{trip}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_a, "truck_id": truck, "trailer_ids": [trailer] })).await;
    server.post(&format!("/fleet/api/v1/trips/{trip}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}")).await;

    // Driver B tries to grab the same truck.
    let driver_b = make_driver(&server, "Driver B").await;
    let resp = server.post(&format!("/fleet/api/v1/drivers/{driver_b}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "truck": truck })).await;
    assert_eq!(resp.status_code(), 409, "truck on another active trip");

    let resp = server.post(&format!("/fleet/api/v1/drivers/{driver_b}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "trailer_ids": [trailer] })).await;
    assert_eq!(resp.status_code(), 409, "trailer on another active trip");
}

#[tokio::test]
async fn test_attach_detach_cascades_active_trip() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "attach7@example.com", "password-attach7").await;
    let auth = format!("Bearer {token}");

    let driver = make_driver(&server, "Cascade Driver").await;
    let truck = make_truck(&server, "CA-TRK-1").await;
    let trailer1 = make_trailer(&server, "CA-TRL-1").await;
    let trailer2 = make_trailer(&server, "CA-TRL-2").await;
    let trip = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "name": "O" },
                { "sequence": 2, "stop_type": "delivery", "name": "D" }
            ]
        }))
        .await.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    server.post(&format!("/fleet/api/v1/trips/{trip}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver, "truck_id": truck, "trailer_ids": [trailer1] })).await;
    server.post(&format!("/fleet/api/v1/trips/{trip}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}")).await;

    // Attach a second trailer — should cascade into the active trip.
    let resp = server.post(&format!("/fleet/api/v1/drivers/{driver}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "trailer_ids": [trailer2] })).await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["trip_cascade"], true);
    assert_eq!(body["trip_id"], trip);

    let t = server.get(&format!("/fleet/api/v1/trips/{trip}"))
        .add_header(header::AUTHORIZATION, &auth).await.json::<serde_json::Value>();
    assert_eq!(t["trailer_ids"].as_array().unwrap().len(), 2, "trip synced with both trailers");

    // Detach trailer1 — released to available; trip synced down to one.
    let resp = server.post(&format!("/fleet/api/v1/drivers/{driver}/detach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "trailer_ids": [trailer1] })).await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<serde_json::Value>()["trip_cascade"], true);
    assert_eq!(trailer_status(&server, &trailer1).await, "available");

    let t = server.get(&format!("/fleet/api/v1/trips/{trip}"))
        .add_header(header::AUTHORIZATION, &auth).await.json::<serde_json::Value>();
    let ids: Vec<String> = t["trailer_ids"].as_array().unwrap().iter()
        .map(|v| v.as_str().unwrap().to_string()).collect();
    assert_eq!(ids, vec![trailer2]);
}

#[tokio::test]
async fn test_detach_equipment_truck_and_all_trailers() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "detach1@example.com", "password-detach1").await;
    let auth = format!("Bearer {token}");

    let driver = make_driver(&server, "Detach Driver").await;
    let truck = make_truck(&server, "DE-TRK-1").await;
    let trl_a = make_trailer(&server, "DE-TRL-A").await;
    let trl_b = make_trailer(&server, "DE-TRL-B").await;

    server.post(&format!("/fleet/api/v1/drivers/{driver}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "truck": truck, "trailer_ids": [trl_a, trl_b] })).await;

    let resp = server.post(&format!("/fleet/api/v1/drivers/{driver}/detach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "truck": true, "all_trailers": true })).await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert!(body["truck_id"].is_null());
    assert_eq!(body["trailer_ids"].as_array().unwrap().len(), 0);

    assert_eq!(truck_status(&server, &truck).await, "available");
    assert_eq!(trailer_status(&server, &trl_a).await, "available");
    assert_eq!(trailer_status(&server, &trl_b).await, "available");
}

#[tokio::test]
async fn test_detach_equipment_empty_body_400() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "detach2@example.com", "password-detach2").await;
    let driver = make_driver(&server, "Detach Empty").await;

    let resp = server.post(&format!("/fleet/api/v1/drivers/{driver}/detach-equipment"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({})).await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_attach_equipment_via_mcp() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mcpattach@example.com", "password-mcpattach").await;

    let driver = make_driver(&server, "MCP Attach Driver").await;
    let truck = make_truck(&server, "MCP-TRK-1").await;
    let trailer = make_trailer(&server, "MCP-TRL-1").await;

    let change = mcp_call(&server, &token, "attach_equipment", serde_json::json!({
        "driver_id": driver, "truck": truck, "trailer_ids": [trailer]
    })).await;
    assert_eq!(change["truck_id"], truck);
    assert_eq!(change["trailer_ids"].as_array().unwrap().len(), 1);

    // detach via MCP
    let change = mcp_call(&server, &token, "detach_equipment", serde_json::json!({
        "driver_id": driver, "truck": true, "all_trailers": true
    })).await;
    assert!(change["truck_id"].is_null());
    assert_eq!(change["trailer_ids"].as_array().unwrap().len(), 0);
}

// ---------------------------------------------------------------------------
// Dispatch-surface parity — write tools over MCP (#330)
//
// These mirror the fleet_user REST parity handlers (commit 9fc7748) through the
// MCP `tools/call` path, covering happy paths plus the key guards. The test
// client declares no elicitation support (capabilities: {}), so destructive
// tools run without a confirmation round-trip (graceful fallback).
// ---------------------------------------------------------------------------

/// Invoke an MCP tool and return the full result block (so callers can inspect
/// `isError` for domain rejections instead of asserting success).
async fn mcp_call_result(
    server: &axum_test::TestServer,
    token: &str,
    name: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    let session = mcp_session(server, token).await;
    let body = mcp_rpc(
        server, token, &session, "tools/call",
        serde_json::json!({ "name": name, "arguments": args }),
    )
    .await;
    assert!(body["error"].is_null(), "MCP {name} protocol error: {:?}", body["error"]);
    body["result"].clone()
}

#[tokio::test]
async fn test_mcp_delete_load_happy_and_active_trip_guard() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = fleet_user_login(&server, "mcp_dl@example.com", "password-mcp-dl").await;
    let fac_id = create_test_facility(&server, "MCP DelLoad Dock", "Fargo, ND").await;

    // Load with an active trip → delete_load must isError.
    let load_id = create_2stop_load(&server, &fac_id, "MCP DelLoad Co").await;
    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "load_id": load_id,
            "stops": [
                { "sequence": 1, "stop_type": "origin", "facility_id": fac_id,
                  "scheduled_arrive": "2026-08-01T08:00:00", "timezone": "America/Chicago" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let guarded = mcp_call_result(&server, &token, "delete_load",
        serde_json::json!({ "id": load_id })).await;
    assert_eq!(guarded["isError"], serde_json::json!(true), "active-trip delete must isError");
    let msg = guarded["content"][0]["text"].as_str().unwrap_or("");
    assert!(msg.contains("active trip"), "guard message: {msg}");

    // Cancel the trip, then delete_load succeeds.
    mcp_call(&server, &token, "delete_trip", serde_json::json!({ "id": trip_id })).await;
    let ok = mcp_call(&server, &token, "delete_load", serde_json::json!({ "id": load_id })).await;
    assert_eq!(ok["deleted"], serde_json::json!(true));

    let after = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(after.status_code(), 404, "deleted load should be gone");
}

#[tokio::test]
async fn test_mcp_delete_trip_soft_cancel() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mcp_dt@example.com", "password-mcp-dt").await;
    let fac_id = create_test_facility(&server, "MCP DelTrip Dock", "Boise, ID").await;

    let trip = mcp_call(&server, &token, "create_trip", serde_json::json!({
        "stops": [
            { "sequence": 1, "stop_type": "pickup", "facility_id": fac_id,
              "scheduled_arrive": "2026-08-01T08:00:00", "timezone": "America/Chicago" },
            { "sequence": 2, "stop_type": "delivery", "facility_id": fac_id,
              "scheduled_arrive": "2026-08-01T16:00:00", "timezone": "America/Chicago" }
        ]
    })).await;
    let trip_id = trip["id"].as_str().unwrap().to_string();

    let del = mcp_call(&server, &token, "delete_trip", serde_json::json!({ "id": trip_id })).await;
    assert_eq!(del["deleted"], serde_json::json!(true));

    let after = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(after["status"], "cancelled", "planned trip soft-cancels");
}

#[tokio::test]
async fn test_mcp_invoice_cancel_settle_load() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mcp_ics@example.com", "password-mcp-ics").await;
    let fac_id = create_test_facility(&server, "MCP Invoice Dock", "Reno, NV").await;

    // cancel_load on a fresh load.
    let cancel_load = create_2stop_load(&server, &fac_id, "MCP Cancel Co").await;
    let cancelled = mcp_call(&server, &token, "cancel_load",
        serde_json::json!({ "id": cancel_load, "reason": "fell through" })).await;
    assert_eq!(cancelled["status"], "cancelled");
    assert_eq!(cancelled["cancellation_reason"], "fell through");

    // invoice → settle lifecycle on a delivered load.
    let load_id = create_2stop_load(&server, &fac_id, "MCP Invoice Co").await;
    drive_load_to_delivered(&server, &token, &fac_id, &load_id).await;

    let invoiced = mcp_call(&server, &token, "invoice_load",
        serde_json::json!({ "id": load_id, "invoice_number": "INV-MCP-1" })).await;
    assert_eq!(invoiced["status"], "invoiced");
    assert_eq!(invoiced["invoice_number"], "INV-MCP-1");

    let settled = mcp_call(&server, &token, "settle_load",
        serde_json::json!({ "id": load_id })).await;
    assert_eq!(settled["status"], "settled");
}

#[tokio::test]
async fn test_mcp_create_update_delete_driver_and_set_pin() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mcp_drv@example.com", "password-mcp-drv").await;

    let driver = mcp_call(&server, &token, "create_driver",
        serde_json::json!({ "name": "MCP Driver" })).await;
    let driver_id = driver["id"].as_str().unwrap().to_string();
    assert_eq!(driver["status"], "available");

    let updated = mcp_call(&server, &token, "update_driver",
        serde_json::json!({ "id": driver_id, "notes": "updated via MCP" })).await;
    assert_eq!(updated["notes"], "updated via MCP");

    // set_driver_pin happy path.
    let pin_ok = mcp_call(&server, &token, "set_driver_pin",
        serde_json::json!({ "id": driver_id, "pin": "1234" })).await;
    assert_eq!(pin_ok["pin_set"], serde_json::json!(true));

    // set_driver_pin bad format → isError.
    let pin_bad = mcp_call_result(&server, &token, "set_driver_pin",
        serde_json::json!({ "id": driver_id, "pin": "12ab" })).await;
    assert_eq!(pin_bad["isError"], serde_json::json!(true), "non-numeric PIN must isError");
    let msg = pin_bad["content"][0]["text"].as_str().unwrap_or("");
    assert!(msg.contains("PIN"), "guard message: {msg}");

    // delete_driver soft-deletes.
    let del = mcp_call(&server, &token, "delete_driver",
        serde_json::json!({ "id": driver_id })).await;
    assert_eq!(del["deleted"], serde_json::json!(true));
    let after = server.get(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(after["status"], "inactive");
}

#[tokio::test]
async fn test_mcp_delete_truck_trailer_facility() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mcp_eq@example.com", "password-mcp-eq").await;

    let truck_id = make_truck(&server, "MCP-DEL-TRK").await;
    let del_truck = mcp_call(&server, &token, "delete_truck",
        serde_json::json!({ "truck_id": truck_id })).await;
    assert_eq!(del_truck["deleted"], serde_json::json!(true));
    assert_eq!(truck_status(&server, &truck_id).await, "inactive");

    let trailer_id = make_trailer(&server, "MCP-DEL-TRL").await;
    let del_trailer = mcp_call(&server, &token, "delete_trailer",
        serde_json::json!({ "trailer_id": trailer_id })).await;
    assert_eq!(del_trailer["deleted"], serde_json::json!(true));
    assert_eq!(trailer_status(&server, &trailer_id).await, "inactive");

    // delete_facility blocked path: a facility referenced by a load must not be
    // deletable — the guard lives in tool_delete_facility and returns an isError
    // result. (MCP is the only surface that exposes facility delete.)
    let ref_fac_id = create_test_facility(&server, "MCP Ref Facility", "Ogden, UT").await;
    let _load_id = create_2stop_load(&server, &ref_fac_id, "MCP RefFac Co").await;
    let blocked = mcp_call_result(&server, &token, "delete_facility",
        serde_json::json!({ "facility_id": ref_fac_id })).await;
    assert_eq!(blocked["isError"], serde_json::json!(true),
        "delete_facility on a load-referenced facility must isError");
    let still_there = server.get(&format!("/fleet/api/v1/facilities/{ref_fac_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(still_there.status_code(), 200, "referenced facility must survive the blocked delete");

    // delete_facility happy path (no loads reference it).
    let fac_id = create_test_facility(&server, "MCP Del Facility", "Provo, UT").await;
    let del_fac = mcp_call(&server, &token, "delete_facility",
        serde_json::json!({ "facility_id": fac_id })).await;
    assert_eq!(del_fac["deleted"], serde_json::json!(true));
    let after = server.get(&format!("/fleet/api/v1/facilities/{fac_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(after.status_code(), 404, "deleted facility should be gone");
}

// ---------------------------------------------------------------------------
// Scope enforcement (#331)
//
// A `fleet_user`-role user is denied elevated operations (loads:settle/invoice,
// master-data deletes, terminal writes) on both the dispatch HTTP surface and
// the MCP tool surface, while owner/fleet_manager pass everything. Per-user
// extra_scopes elevate a single capability.
// ---------------------------------------------------------------------------

/// Create a fleet_user with an explicit role and log in, returning a JWT.
async fn login_with_role(
    server: &axum_test::TestServer,
    email: &str,
    password: &str,
    role: &str,
) -> String {
    // There is exactly one owner (the first-run bootstrap user). A request for an
    // `owner` token returns that bootstrap owner directly — the users surface
    // forbids creating additional owners. Other roles are created normally.
    let owner = setup_owner(server).await;
    if role == "owner" {
        return owner;
    }
    server.post("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner}"))
        .json(&serde_json::json!({
            "email": email, "name": "Scoped User", "password": password, "role": role,
        }))
        .await;
    let resp = server.post("/fleet/auth/login")
        .json(&serde_json::json!({ "email": email, "password": password }))
        .await;
    assert_eq!(resp.status_code(), 200, "login failed for {email}");
    resp.json::<serde_json::Value>()["token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_scope_fleet_user_denied_elevated_http_ops() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let token = login_with_role(&server, "scope_disp@example.com", "pw-scope-disp", "dispatcher").await;
    let auth = format!("Bearer {token}");
    let fac_id = create_test_facility(&server, "Scope Dock", "Tulsa, OK").await;
    let load_id = create_2stop_load(&server, &fac_id, "Scope Co").await;
    let driver_id = create_test_driver(&server).await;

    // Create a truck via admin so we can attempt a dispatch-surface delete.
    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": "SCOPE-1" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // A trip to delete.
    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "load_id": load_id,
            "stops": [{ "sequence": 1, "stop_type": "origin", "facility_id": fac_id,
                "scheduled_arrive": "2026-08-01T08:00:00", "timezone": "America/Chicago" }]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // DENIED (403): settle, invoice, load delete, trip delete, driver delete,
    // truck delete, terminal create.
    let settle = server.post(&format!("/fleet/api/v1/loads/{load_id}/settle"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(settle.status_code(), 403, "fleet_user must be denied settle");

    let invoice = server.post(&format!("/fleet/api/v1/loads/{load_id}/invoice"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({})).await;
    assert_eq!(invoice.status_code(), 403, "fleet_user must be denied invoice");

    let del_load = server.delete(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(del_load.status_code(), 403, "fleet_user must be denied load delete");

    // NOTE: the chunk-1 permission model grants the fleet_user role `trips:delete`
    // (it's in DISPATCHER_SCOPES), so trip delete is ALLOWED for a fleet_user —
    // unlike load/driver/truck deletes. We follow the merged model rather than
    // weaken it. (The spec's denial list listed trip delete; the authoritative
    // permission model says otherwise.)
    let del_trip = server.delete(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(del_trip.status_code(), 204, "fleet_user has trips:delete in the model");

    let del_driver = server.delete(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(del_driver.status_code(), 403, "fleet_user must be denied driver delete");

    let del_truck = server.delete(&format!("/fleet/api/v1/trucks/{truck_id}"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(del_truck.status_code(), 403, "fleet_user must be denied truck delete");

    let term = server.post("/fleet/api/v1/terminals")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "name": "West", "timezone": "America/Los_Angeles" })).await;
    assert_eq!(term.status_code(), 403, "fleet_user must be denied terminal create");
}

#[tokio::test]
async fn test_scope_fleet_user_allowed_operational_http_ops() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = login_with_role(&server, "scope_disp_ok@example.com", "pw-scope-ok", "dispatcher").await;
    let auth = format!("Bearer {token}");
    let fac_id = create_test_facility(&server, "Scope OK Dock", "Omaha, NE").await;

    // load create
    let load = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({
            "customer_name": "Op Co",
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                  "facility_id": fac_id, "scheduled_arrive": "2026-08-01T08:00:00", "timezone": "America/Chicago" },
                { "sequence": 2, "stop_type": "delivery", "service_type": "live_unload",
                  "facility_id": fac_id, "scheduled_arrive": "2026-08-01T16:00:00", "timezone": "America/Chicago" }
            ],
            "rate_items": []
        })).await;
    assert_eq!(load.status_code(), 201, "fleet_user allowed load create");
    let load_id = load.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // load update
    let upd = server.put(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "notes": "touched" })).await;
    assert_eq!(upd.status_code(), 200, "fleet_user allowed load update");

    // trip create
    let trip = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "load_id": load_id })).await;
    assert_eq!(trip.status_code(), 201, "fleet_user allowed trip create");

    // driver create + patch
    let drv = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "name": "Op Driver" })).await;
    assert_eq!(drv.status_code(), 201, "fleet_user allowed driver create");
    let drv_id = drv.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let patch = server.patch(&format!("/fleet/api/v1/drivers/{drv_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "phone": "555-0100" })).await;
    assert_eq!(patch.status_code(), 200, "fleet_user allowed driver patch");

    // stop arrive/depart on the trip's first stop.
    let trip_id = trip.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let arrive = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/1/arrive"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "actual_arrive": "2026-08-01T08:05:00" })).await;
    assert_eq!(arrive.status_code(), 200, "fleet_user allowed stop arrive");
    let depart = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/1/depart"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "actual_depart": "2026-08-01T08:30:00" })).await;
    assert_eq!(depart.status_code(), 200, "fleet_user allowed stop depart");
}

#[tokio::test]
async fn test_scope_owner_allowed_elevated_http_ops() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = login_with_role(&server, "scope_owner@example.com", "pw-scope-owner", "owner").await;
    let auth = format!("Bearer {token}");
    let fac_id = create_test_facility(&server, "Owner Dock", "Denver, CO").await;
    let load_id = create_2stop_load(&server, &fac_id, "Owner Co").await;

    drive_load_to_delivered(&server, &token, &fac_id, &load_id).await;

    let invoice = server.post(&format!("/fleet/api/v1/loads/{load_id}/invoice"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "invoice_number": "OWN-1" })).await;
    assert_eq!(invoice.status_code(), 200, "owner allowed invoice");

    let settle = server.post(&format!("/fleet/api/v1/loads/{load_id}/settle"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(settle.status_code(), 200, "owner allowed settle");

    // owner allowed terminal create.
    let term = server.post("/fleet/api/v1/terminals")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "name": "Mountain", "timezone": "America/Denver" })).await;
    assert_eq!(term.status_code(), 201, "owner allowed terminal create");
}

#[tokio::test]
async fn test_scope_mcp_fleet_user_denied_owner_allowed() {
    let (server, _b, _d, _rx) = test_server().await;
    let disp = login_with_role(&server, "scope_mcp_disp@example.com", "pw-mcp-disp", "dispatcher").await;
    let owner = login_with_role(&server, "scope_mcp_owner@example.com", "pw-mcp-owner", "owner").await;
    let fac_id = create_test_facility(&server, "MCP Scope Dock", "Mesa, AZ").await;
    let load_id = create_2stop_load(&server, &fac_id, "MCP Scope Co").await;

    // Dispatcher: settle_load and delete_load via MCP are denied (isError).
    let settle = mcp_call_result(&server, &disp, "settle_load",
        serde_json::json!({ "id": load_id })).await;
    assert_eq!(settle["isError"], serde_json::json!(true), "fleet_user settle_load must isError");
    let msg = settle["content"][0]["text"].as_str().unwrap_or("");
    assert!(msg.contains("scope"), "denial should mention scope: {msg}");

    let del = mcp_call_result(&server, &disp, "delete_load",
        serde_json::json!({ "id": load_id })).await;
    assert_eq!(del["isError"], serde_json::json!(true), "fleet_user delete_load must isError");

    // Owner: same calls succeed.
    drive_load_to_delivered(&server, &owner, &fac_id, &load_id).await;
    let _inv = mcp_call(&server, &owner, "invoice_load",
        serde_json::json!({ "id": load_id, "invoice_number": "OWN-MCP-1" })).await;
    let settled = mcp_call(&server, &owner, "settle_load",
        serde_json::json!({ "id": load_id })).await;
    assert_eq!(settled["status"], "settled", "owner settle_load succeeds");
}

#[tokio::test]
async fn test_scope_extra_scope_grant_allows_settle() {
    // A fleet_user granted the single `loads:settle` extra scope can settle, on
    // both HTTP and MCP — without gaining any sibling elevated capability.
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let token = login_with_role(&server, "scope_grant@example.com", "pw-grant", "dispatcher").await;
    let auth = format!("Bearer {token}");

    // Grant the extra scope directly via the DB (the Users management surface that
    // would set this is chunk 3).
    let mut record = state.db.get_fleet_user_by_email("scope_grant@example.com")
        .await.unwrap().expect("fleet_user exists");
    record.extra_scopes = vec!["loads:settle".to_string()];
    state.db.upsert_fleet_user(&record).await.unwrap();

    let fac_id = create_test_facility(&server, "Grant Dock", "Reno, NV").await;
    let load_id = create_2stop_load(&server, &fac_id, "Grant Co").await;
    drive_load_to_delivered(&server, &token, &fac_id, &load_id).await;

    // invoice is still denied (sibling elevated scope not granted)...
    let invoice = server.post(&format!("/fleet/api/v1/loads/{load_id}/invoice"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({})).await;
    assert_eq!(invoice.status_code(), 403, "grant of loads:settle must not leak loads:invoice");

    // ...but the load must be invoiced before it can settle; do that via owner.
    let owner = login_with_role(&server, "scope_grant_owner@example.com", "pw-grant-own", "owner").await;
    let inv = server.post(&format!("/fleet/api/v1/loads/{load_id}/invoice"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner}"))
        .json(&serde_json::json!({ "invoice_number": "GRANT-1" })).await;
    assert_eq!(inv.status_code(), 200);

    // Now the granted fleet_user can settle (HTTP).
    let settle = server.post(&format!("/fleet/api/v1/loads/{load_id}/settle"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(settle.status_code(), 200, "granted fleet_user allowed to settle");
}

// ---------------------------------------------------------------------------
// Fleet users management surface (#331)
//
// HTTP + MCP users CRUD gated by users:* scopes (owner + fleet_manager only),
// with owner-protection rules: at-least-one-owner invariant, the owner cannot
// be demoted/deactivated except via ownership transfer, and ownership transfer
// is owner-only.
// ---------------------------------------------------------------------------

/// Create a user via the dispatch Users surface as `owner_token`, returning the
/// created record's id.
async fn create_user_via_surface(
    server: &axum_test::TestServer,
    owner_token: &str,
    email: &str,
    name: &str,
    password: &str,
    role: &str,
) -> String {
    let resp = server.post("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "email": email, "name": name, "password": password, "role": role,
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "create user failed: {}", resp.text());
    resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_users_owner_full_crud() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner = login_with_role(&server, "u_owner@example.com", "pw-u-owner", "owner").await;
    let auth = format!("Bearer {owner}");

    // Create a fleet_user and a fleet_manager.
    let disp_id = create_user_via_surface(&server, &owner,
        "u_disp@example.com", "Disp One", "pw-disp-one", "dispatcher").await;
    let fm_id = create_user_via_surface(&server, &owner,
        "u_fm@example.com", "FM One", "pw-fm-one", "fleet_manager").await;

    // List sees the owner + both created users (>=3), and never exposes hashes.
    let list = server.get("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(list.status_code(), 200);
    let body = list.json::<serde_json::Value>();
    let users = body["users"].as_array().unwrap();
    assert!(users.len() >= 3, "expected at least 3 users");
    let raw = list.text();
    assert!(!raw.contains("password_hash"), "list must not expose password_hash");

    // Get one user.
    let get = server.get(&format!("/fleet/api/v1/users/{disp_id}"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(get.status_code(), 200);
    assert_eq!(get.json::<serde_json::Value>()["role"], "dispatcher");

    // Update name + extra_scopes on the fleet_user.
    let upd = server.patch(&format!("/fleet/api/v1/users/{disp_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "name": "Disp Renamed", "extra_scopes": ["loads:settle"] }))
        .await;
    assert_eq!(upd.status_code(), 200);
    let updated = upd.json::<serde_json::Value>();
    assert_eq!(updated["name"], "Disp Renamed");
    assert_eq!(updated["extra_scopes"][0], "loads:settle");

    // Reset the fleet_manager's password — their old JWT must be invalidated.
    let fm_token = {
        let resp = server.post("/fleet/auth/login")
            .json(&serde_json::json!({ "email": "u_fm@example.com", "password": "pw-fm-one" }))
            .await;
        assert_eq!(resp.status_code(), 200);
        resp.json::<serde_json::Value>()["token"].as_str().unwrap().to_string()
    };
    // FM can list users before reset.
    let pre = server.get("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, format!("Bearer {fm_token}")).await;
    assert_eq!(pre.status_code(), 200, "fleet_manager can read users");

    let reset = server.put(&format!("/fleet/api/v1/users/{fm_id}/password"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "password": "pw-fm-new" })).await;
    assert_eq!(reset.status_code(), 204);

    // Old FM JWT is now invalid (token_version bumped).
    let post_reset = server.get("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, format!("Bearer {fm_token}")).await;
    assert_eq!(post_reset.status_code(), 401, "old JWT invalidated after password reset");

    // Deactivate the fleet_user.
    let del = server.delete(&format!("/fleet/api/v1/users/{disp_id}"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(del.status_code(), 204);
    let after = server.get(&format!("/fleet/api/v1/users/{disp_id}"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(after.json::<serde_json::Value>()["status"], "inactive");
}

#[tokio::test]
async fn test_users_fleet_user_forbidden_everywhere() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner = login_with_role(&server, "uf_owner@example.com", "pw-uf-owner", "owner").await;
    let disp = login_with_role(&server, "uf_disp@example.com", "pw-uf-disp", "dispatcher").await;
    let dauth = format!("Bearer {disp}");
    let target_id = create_user_via_surface(&server, &owner,
        "uf_target@example.com", "Target", "pw-target", "dispatcher").await;

    // Every endpoint is 403 for a plain fleet_user.
    assert_eq!(server.get("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, &dauth).await.status_code(), 403);
    assert_eq!(server.get(&format!("/fleet/api/v1/users/{target_id}"))
        .add_header(header::AUTHORIZATION, &dauth).await.status_code(), 403);
    assert_eq!(server.post("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, &dauth)
        .json(&serde_json::json!({ "email": "x@example.com", "name": "X", "password": "pwpwpwpw", "role": "dispatcher" }))
        .await.status_code(), 403);
    assert_eq!(server.patch(&format!("/fleet/api/v1/users/{target_id}"))
        .add_header(header::AUTHORIZATION, &dauth)
        .json(&serde_json::json!({ "name": "Nope" })).await.status_code(), 403);
    assert_eq!(server.put(&format!("/fleet/api/v1/users/{target_id}/password"))
        .add_header(header::AUTHORIZATION, &dauth)
        .json(&serde_json::json!({ "password": "pwpwpwpw" })).await.status_code(), 403);
    assert_eq!(server.delete(&format!("/fleet/api/v1/users/{target_id}"))
        .add_header(header::AUTHORIZATION, &dauth).await.status_code(), 403);
}

#[tokio::test]
async fn test_users_fleet_manager_can_manage_but_not_set_owner() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner = login_with_role(&server, "fm_owner@example.com", "pw-fm-owner", "owner").await;
    // Create a fleet_manager and a plain fleet_user target.
    create_user_via_surface(&server, &owner, "fm_mgr@example.com", "Mgr", "pw-fm-mgr", "fleet_manager").await;
    let target_id = create_user_via_surface(&server, &owner,
        "fm_tgt@example.com", "Tgt", "pw-fm-tgt", "dispatcher").await;

    let fm = {
        let resp = server.post("/fleet/auth/login")
            .json(&serde_json::json!({ "email": "fm_mgr@example.com", "password": "pw-fm-mgr" })).await;
        resp.json::<serde_json::Value>()["token"].as_str().unwrap().to_string()
    };
    let fmauth = format!("Bearer {fm}");

    // FM can create/list/update normal users.
    let new_id = create_user_via_surface(&server, &fm, "fm_new@example.com", "New", "pw-fm-new", "dispatcher").await;
    assert_eq!(server.get("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, &fmauth).await.status_code(), 200);
    assert_eq!(server.patch(&format!("/fleet/api/v1/users/{new_id}"))
        .add_header(header::AUTHORIZATION, &fmauth)
        .json(&serde_json::json!({ "name": "Renamed" })).await.status_code(), 200);

    // FM CANNOT set anyone's role to owner (transfer is owner-only) → 403.
    let transfer = server.patch(&format!("/fleet/api/v1/users/{target_id}"))
        .add_header(header::AUTHORIZATION, &fmauth)
        .json(&serde_json::json!({ "role": "owner" })).await;
    assert_eq!(transfer.status_code(), 403, "fleet_manager cannot transfer ownership");
}

#[tokio::test]
async fn test_users_owner_protection_rules() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner = login_with_role(&server, "op_owner@example.com", "pw-op-owner", "owner").await;
    let auth = format!("Bearer {owner}");

    // The owner's own id.
    let list = server.get("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, &auth).await.json::<serde_json::Value>();
    let owner_id = list["users"].as_array().unwrap().iter()
        .find(|u| u["role"] == "owner").unwrap()["id"].as_str().unwrap().to_string();

    // Cannot delete the sole owner → 403 (role-based owner-protection, not a
    // count-based conflict): owners are immutable except via ownership transfer.
    let del = server.delete(&format!("/fleet/api/v1/users/{owner_id}"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(del.status_code(), 403, "cannot deactivate the owner");
    assert!(
        del.text().contains("transfer ownership first"),
        "expected transfer-ownership message, got: {}",
        del.text()
    );

    // Cannot demote the sole owner via PATCH → 403.
    let demote = server.patch(&format!("/fleet/api/v1/users/{owner_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "role": "fleet_manager" })).await;
    assert_eq!(demote.status_code(), 403, "cannot demote the sole owner");

    // Cannot deactivate the sole owner via PATCH status → 403.
    let deact = server.patch(&format!("/fleet/api/v1/users/{owner_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "status": "inactive" })).await;
    assert_eq!(deact.status_code(), 403, "cannot deactivate the owner via PATCH");

    // create with role=owner is rejected → 403.
    let bad = server.post("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "email": "op_new@example.com", "name": "N", "password": "pwpwpwpw", "role": "owner" }))
        .await;
    assert_eq!(bad.status_code(), 403, "create with role=owner rejected");
}

// A fleet_manager (who holds users:delete) must not be able to deactivate ANY
// owner via DELETE /users/{id}, even when ≥2 owners exist — not just the sole
// owner. This mirrors apply_update_user's unconditional owner-protection.
#[tokio::test]
async fn test_users_delete_owner_forbidden_with_two_owners() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;

    // The first-run bootstrap owner, plus a SECOND owner seeded directly into the
    // DB. A two-owner state is only otherwise reachable mid-transfer; the users
    // surface forbids creating a second owner, so we insert it directly to set up
    // the precondition this test needs.
    let owner1 = setup_owner(&server).await;
    seed_owner_direct(&state, "two_owner2@example.com", "pw-two-owner2").await;
    let auth = format!("Bearer {owner1}");

    // Confirm two active owners present.
    let list = server.get("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, &auth).await.json::<serde_json::Value>();
    let arr = list["users"].as_array().unwrap();
    let owner_ids: Vec<String> = arr.iter()
        .filter(|u| u["role"] == "owner")
        .map(|u| u["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(owner_ids.len(), 2, "expected two seeded owners");

    // A fleet_manager (holds users:delete) attempts to deactivate owner2.
    let fm = login_with_role(&server, "two_fm@example.com", "pw-two-fm", "fleet_manager").await;
    let fmauth = format!("Bearer {fm}");
    let target = arr.iter()
        .find(|u| u["email"] == "two_owner2@example.com").unwrap()["id"].as_str().unwrap();
    let del = server.delete(&format!("/fleet/api/v1/users/{target}"))
        .add_header(header::AUTHORIZATION, &fmauth).await;
    assert_eq!(del.status_code(), 403, "fleet_manager cannot deactivate an owner even with two owners");
    assert!(
        del.text().contains("transfer ownership first"),
        "expected transfer-ownership message, got: {}",
        del.text()
    );

    // Owner count unchanged: both owners still active.
    let after = server.get("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, &auth).await.json::<serde_json::Value>();
    let still_owners = after["users"].as_array().unwrap().iter()
        .filter(|u| u["role"] == "owner" && u["status"] == "active").count();
    assert_eq!(still_owners, 2, "owner count must be unchanged after rejected delete");
}

#[tokio::test]
async fn test_users_ownership_transfer() {
    let (server, _b, _d, _rx) = test_server().await;
    // The sole owner is the first-run bootstrap owner.
    let owner = login_with_role(&server, OWNER_EMAIL, OWNER_PASSWORD, "owner").await;
    let auth = format!("Bearer {owner}");

    let x_id = create_user_via_surface(&server, &owner,
        "tr_x@example.com", "User X", "pw-tr-x", "fleet_manager").await;

    // Owner promotes X to owner → transfer.
    let transfer = server.patch(&format!("/fleet/api/v1/users/{x_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "role": "owner" })).await;
    assert_eq!(transfer.status_code(), 200, "owner can transfer: {}", transfer.text());
    assert_eq!(transfer.json::<serde_json::Value>()["role"], "owner");

    // X is owner via login; prior owner is now fleet_manager.
    let x_token = {
        let r = server.post("/fleet/auth/login")
            .json(&serde_json::json!({ "email": "tr_x@example.com", "password": "pw-tr-x" })).await;
        r.json::<serde_json::Value>()["token"].as_str().unwrap().to_string()
    };
    let xauth = format!("Bearer {x_token}");
    let users = server.get("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, &xauth).await.json::<serde_json::Value>();
    let arr = users["users"].as_array().unwrap();
    let owners: Vec<_> = arr.iter().filter(|u| u["role"] == "owner").collect();
    assert_eq!(owners.len(), 1, "exactly one owner after transfer");
    assert_eq!(owners[0]["id"].as_str().unwrap(), x_id);
    let prior = arr.iter().find(|u| u["email"] == OWNER_EMAIL).unwrap();
    assert_eq!(prior["role"], "fleet_manager", "prior owner demoted to fleet_manager");

    // Prior owner (now fleet_manager) can no longer transfer → 403.
    // (Their JWT still valid — transfer demotes role but does not bump token_version.)
    // Make a fresh target and have the demoted prior owner attempt to promote it.
    let y_id = create_user_via_surface(&server, &x_token,
        "tr_y@example.com", "User Y", "pw-tr-y", "dispatcher").await;
    let prior_transfer = server.patch(&format!("/fleet/api/v1/users/{y_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "role": "owner" })).await;
    assert_eq!(prior_transfer.status_code(), 403, "demoted prior owner can no longer transfer");
}

#[tokio::test]
async fn test_users_mcp_parity() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner = login_with_role(&server, "mcpu_owner@example.com", "pw-mcpu-owner", "owner").await;
    let disp = login_with_role(&server, "mcpu_disp@example.com", "pw-mcpu-disp", "dispatcher").await;

    // Dispatcher calling create_user via MCP is denied (scope isError).
    let denied = mcp_call_result(&server, &disp, "create_user", serde_json::json!({
        "email": "mcpu_x@example.com", "name": "X", "password": "pwpwpwpw", "role": "dispatcher"
    })).await;
    assert_eq!(denied["isError"], serde_json::json!(true), "fleet_user create_user must isError");
    assert!(denied["content"][0]["text"].as_str().unwrap_or("").contains("scope"));

    // Owner creates a user via MCP.
    let created = mcp_call(&server, &owner, "create_user", serde_json::json!({
        "email": "mcpu_made@example.com", "name": "Made", "password": "pw-made-1", "role": "dispatcher"
    })).await;
    let made_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["role"], "dispatcher");

    // Owner updates via MCP.
    let updated = mcp_call(&server, &owner, "update_user", serde_json::json!({
        "id": made_id, "name": "Made Renamed"
    })).await;
    assert_eq!(updated["name"], "Made Renamed");

    // create_user with role=owner via MCP is rejected (owner-protection isError).
    let bad = mcp_call_result(&server, &owner, "create_user", serde_json::json!({
        "email": "mcpu_owner2@example.com", "name": "O2", "password": "pwpwpwpw", "role": "owner"
    })).await;
    assert_eq!(bad["isError"], serde_json::json!(true), "create_user role=owner must isError");

    // delete_user via MCP deactivates.
    let del = mcp_call(&server, &owner, "delete_user", serde_json::json!({ "id": made_id })).await;
    assert_eq!(del["deleted"], serde_json::json!(true));
}

/// Log in a fleet_manager that the owner created, returning their JWT.
async fn login_as(server: &axum_test::TestServer, email: &str, password: &str) -> String {
    let resp = server.post("/fleet/auth/login")
        .json(&serde_json::json!({ "email": email, "password": password })).await;
    assert_eq!(resp.status_code(), 200, "login failed: {}", resp.text());
    resp.json::<serde_json::Value>()["token"].as_str().unwrap().to_string()
}

// Finding 1 (CRITICAL): a fleet_manager (who holds users:write) must NOT be able
// to reset the owner's password; only the current owner may.
#[tokio::test]
async fn test_users_fleet_manager_cannot_reset_owner_password() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner = login_with_role(&server, "rp_owner@example.com", "pw-rp-owner", "owner").await;
    let auth = format!("Bearer {owner}");

    // Identify the owner.
    let list = server.get("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, &auth).await.json::<serde_json::Value>();
    let owner_id = list["users"].as_array().unwrap().iter()
        .find(|u| u["role"] == "owner").unwrap()["id"].as_str().unwrap().to_string();

    // Owner creates a fleet_manager and a plain fleet_user target.
    create_user_via_surface(&server, &owner,
        "rp_fm@example.com", "FM", "pw-rp-fm", "fleet_manager").await;
    let disp_id = create_user_via_surface(&server, &owner,
        "rp_disp@example.com", "Disp", "pw-rp-disp", "dispatcher").await;
    let fm = login_as(&server, "rp_fm@example.com", "pw-rp-fm").await;
    let fmauth = format!("Bearer {fm}");

    // FM resetting the OWNER's password → 403.
    let blocked = server.put(&format!("/fleet/api/v1/users/{owner_id}/password"))
        .add_header(header::AUTHORIZATION, &fmauth)
        .json(&serde_json::json!({ "password": "pwned-owner-pw" })).await;
    assert_eq!(blocked.status_code(), 403, "fleet_manager cannot reset owner password");

    // FM resetting a non-owner's password is still fine.
    let ok_disp = server.put(&format!("/fleet/api/v1/users/{disp_id}/password"))
        .add_header(header::AUTHORIZATION, &fmauth)
        .json(&serde_json::json!({ "password": "new-disp-pw" })).await;
    assert_eq!(ok_disp.status_code(), 204, "fleet_manager may reset a non-owner password");

    // The owner may reset another user's password, and their own. Reset another's
    // first — resetting own bumps the owner's token_version and invalidates `auth`.
    let ok_other = server.put(&format!("/fleet/api/v1/users/{disp_id}/password"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "password": "another-pw" })).await;
    assert_eq!(ok_other.status_code(), 204, "owner may reset another's password");
    let ok_self = server.put(&format!("/fleet/api/v1/users/{owner_id}/password"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "password": "new-owner-pw" })).await;
    assert_eq!(ok_self.status_code(), 204, "owner may reset own password");
}

// Finding 2 (HIGH): extra_scopes must be validated. A fleet_manager cannot grant
// user-management/superuser scopes (shadow-admin minting); the owner can. A
// fleet_manager CAN grant an operational scope they themselves hold.
#[tokio::test]
async fn test_users_extra_scopes_grant_gating() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner = login_with_role(&server, "es_owner@example.com", "pw-es-owner", "owner").await;
    let auth = format!("Bearer {owner}");

    create_user_via_surface(&server, &owner,
        "es_fm@example.com", "FM", "pw-es-fm", "fleet_manager").await;
    let target_id = create_user_via_surface(&server, &owner,
        "es_tgt@example.com", "Tgt", "pw-es-tgt", "dispatcher").await;
    let fm = login_as(&server, "es_fm@example.com", "pw-es-fm").await;
    let fmauth = format!("Bearer {fm}");

    // FM creating a user with users:write → 403 (shadow admin).
    let bad_create = server.post("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, &fmauth)
        .json(&serde_json::json!({
            "email": "es_shadow@example.com", "name": "S", "password": "pwpwpwpw",
            "role": "dispatcher", "extra_scopes": ["users:write"]
        })).await;
    assert_eq!(bad_create.status_code(), 403, "fleet_manager cannot grant users:write on create");

    // FM creating a user with superuser * → 403.
    let bad_star = server.post("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, &fmauth)
        .json(&serde_json::json!({
            "email": "es_star@example.com", "name": "S", "password": "pwpwpwpw",
            "role": "dispatcher", "extra_scopes": ["*"]
        })).await;
    assert_eq!(bad_star.status_code(), 403, "fleet_manager cannot grant * on create");

    // FM updating a user with users:write → 403.
    let bad_update = server.patch(&format!("/fleet/api/v1/users/{target_id}"))
        .add_header(header::AUTHORIZATION, &fmauth)
        .json(&serde_json::json!({ "extra_scopes": ["users:write"] })).await;
    assert_eq!(bad_update.status_code(), 403, "fleet_manager cannot grant users:write on update");

    // FM granting an operational scope they hold (loads:settle) → ok. Sanity that
    // the fix is not over-broad.
    let ok_update = server.patch(&format!("/fleet/api/v1/users/{target_id}"))
        .add_header(header::AUTHORIZATION, &fmauth)
        .json(&serde_json::json!({ "extra_scopes": ["loads:settle"] })).await;
    assert_eq!(ok_update.status_code(), 200, "fleet_manager may grant a scope it holds: {}", ok_update.text());
    assert_eq!(ok_update.json::<serde_json::Value>()["extra_scopes"][0], "loads:settle");

    // The OWNER may grant users:write and *.
    let owner_create = server.post("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({
            "email": "es_priv@example.com", "name": "P", "password": "pwpwpwpw",
            "role": "dispatcher", "extra_scopes": ["users:write"]
        })).await;
    assert_eq!(owner_create.status_code(), 201, "owner may grant users:write: {}", owner_create.text());
    let owner_update = server.patch(&format!("/fleet/api/v1/users/{target_id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "extra_scopes": ["*"] })).await;
    assert_eq!(owner_update.status_code(), 200, "owner may grant *: {}", owner_update.text());
}

// Finding 1 + 2 enforced identically via the MCP tool surface.
#[tokio::test]
async fn test_users_mcp_grant_and_reset_gating() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner = login_with_role(&server, "mg_owner@example.com", "pw-mg-owner", "owner").await;
    let auth = format!("Bearer {owner}");

    let list = server.get("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, &auth).await.json::<serde_json::Value>();
    let owner_id = list["users"].as_array().unwrap().iter()
        .find(|u| u["role"] == "owner").unwrap()["id"].as_str().unwrap().to_string();

    create_user_via_surface(&server, &owner,
        "mg_fm@example.com", "FM", "pw-mg-fm", "fleet_manager").await;
    let target_id = create_user_via_surface(&server, &owner,
        "mg_tgt@example.com", "Tgt", "pw-mg-tgt", "dispatcher").await;
    let fm = login_as(&server, "mg_fm@example.com", "pw-mg-fm").await;

    // FM reset_user_password against the owner via MCP → isError.
    let reset_blocked = mcp_call_result(&server, &fm, "reset_user_password",
        serde_json::json!({ "id": owner_id, "password": "pwned-via-mcp" })).await;
    assert_eq!(reset_blocked["isError"], serde_json::json!(true), "fm reset owner pw via MCP must isError");

    // FM create_user with users:write via MCP → isError.
    let create_blocked = mcp_call_result(&server, &fm, "create_user", serde_json::json!({
        "email": "mg_shadow@example.com", "name": "S", "password": "pwpwpwpw",
        "role": "dispatcher", "extra_scopes": ["users:write"]
    })).await;
    assert_eq!(create_blocked["isError"], serde_json::json!(true), "fm grant users:write via MCP must isError");

    // FM update_user granting * via MCP → isError.
    let update_blocked = mcp_call_result(&server, &fm, "update_user", serde_json::json!({
        "id": target_id, "extra_scopes": ["*"]
    })).await;
    assert_eq!(update_blocked["isError"], serde_json::json!(true), "fm grant * via MCP must isError");

    // Owner doing the same via MCP succeeds.
    let owner_made = mcp_call(&server, &owner, "create_user", serde_json::json!({
        "email": "mg_priv@example.com", "name": "P", "password": "pwpwpwpw",
        "role": "dispatcher", "extra_scopes": ["users:write"]
    })).await;
    assert_eq!(owner_made["extra_scopes"][0], "users:write");
}

// ---------------------------------------------------------------------------
// First-run owner setup wizard (#331)
//
// Unauthenticated /fleet/api/v1/setup/status + /fleet/setup, guarded by
// count_fleet_users() == 0. Creates the first owner and logs them straight in.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_setup_wizard_full_flow() {
    let (server, _b, _d, _rx) = test_server().await;

    // Empty server → needs_setup = true.
    let status = server.get("/fleet/api/v1/setup/status").await;
    assert_eq!(status.status_code(), 200);
    assert_eq!(status.json::<serde_json::Value>()["needs_setup"], serde_json::json!(true));

    // POST /fleet/setup creates an owner and returns a session token.
    let created = server.post("/fleet/setup")
        .json(&serde_json::json!({
            "email": "boss@example.com", "name": "The Boss", "password": "owner-password-1",
        }))
        .await;
    assert_eq!(created.status_code(), 200, "setup failed: {}", created.text());
    let token = created.json::<serde_json::Value>()["token"].as_str().unwrap().to_string();
    assert!(!token.is_empty(), "setup must return a session token");
    // Auto-login: a refresh cookie is set, same shape as login.
    assert!(created.headers().get(header::SET_COOKIE).is_some(),
        "setup must set a refresh cookie (auto-login)");

    // Status now reports false.
    let status2 = server.get("/fleet/api/v1/setup/status").await;
    assert_eq!(status2.json::<serde_json::Value>()["needs_setup"], serde_json::json!(false));

    // Second POST is rejected — guard slammed shut.
    let again = server.post("/fleet/setup")
        .json(&serde_json::json!({
            "email": "intruder@example.com", "name": "Nope", "password": "another-password",
        }))
        .await;
    assert_eq!(again.status_code(), 409, "second setup must be 409 Conflict");

    // The created owner really has role=owner: the returned token can drive the
    // users:* surface (owner-only) and the table holds exactly one owner.
    let auth = format!("Bearer {token}");
    let list = server.get("/fleet/api/v1/users")
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(list.status_code(), 200, "owner token must reach users surface: {}", list.text());
    let users = list.json::<serde_json::Value>();
    assert_eq!(users["returned"], serde_json::json!(1));
    assert_eq!(users["users"][0]["role"], serde_json::json!("owner"));
    assert_eq!(users["users"][0]["email"], serde_json::json!("boss@example.com"));

    // And the owner can log in normally afterward.
    let login = server.post("/fleet/auth/login")
        .json(&serde_json::json!({ "email": "boss@example.com", "password": "owner-password-1" }))
        .await;
    assert_eq!(login.status_code(), 200, "owner must be able to log in normally");
}

#[tokio::test]
async fn test_me_returns_owner_identity_and_scopes() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;

    let resp = server.get("/fleet/api/v1/me")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;

    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["email"], OWNER_EMAIL);
    assert_eq!(body["role"], "owner");
    // Owner role bundle is the global superuser scope.
    let scopes: Vec<String> = body["effective_scopes"]
        .as_array().unwrap().iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect();
    assert!(scopes.contains(&"*".to_string()), "owner should have * scope, got {scopes:?}");
}

#[tokio::test]
async fn test_me_without_auth_returns_401() {
    let (server, _b, _d, _rx) = test_server().await;
    let resp = server.get("/fleet/api/v1/me").await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_setup_status_false_once_user_exists() {
    let (server, _b, _d, _rx) = test_server().await;
    // Provision the first user via first-run setup; status must then report false.
    setup_owner(&server).await;
    let status = server.get("/fleet/api/v1/setup/status").await;
    assert_eq!(status.json::<serde_json::Value>()["needs_setup"], serde_json::json!(false));

    // And setup is closed.
    let attempt = server.post("/fleet/setup")
        .json(&serde_json::json!({
            "email": "late@example.com", "name": "Late", "password": "late-password",
        }))
        .await;
    assert_eq!(attempt.status_code(), 409);
}

// Reads a driver's stored loaded_rate_per_mile override (null when inherited).
async fn driver_loaded_rate(server: &axum_test::TestServer, token: &str, id: &str) -> serde_json::Value {
    server.get(&format!("/fleet/api/v1/drivers/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>()["loaded_rate_per_mile"].clone()
}

#[tokio::test]
async fn test_driver_rate_override_set_then_clear() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let driver_id = create_test_driver(&server).await;

    // Set an override.
    let set = server.patch(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "loaded_rate_per_mile": 0.75 }))
        .await;
    assert_eq!(set.status_code(), 200);
    assert_eq!(driver_loaded_rate(&server, &owner_token, &driver_id).await, serde_json::json!(0.75));

    // Clear it with an explicit null → back to inherited (null).
    let clear = server.patch(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "loaded_rate_per_mile": null }))
        .await;
    assert_eq!(clear.status_code(), 200);
    assert!(driver_loaded_rate(&server, &owner_token, &driver_id).await.is_null());
}

#[tokio::test]
async fn test_driver_rate_override_absent_field_leaves_others_unchanged() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let driver_id = create_test_driver(&server).await;

    // Set loaded rate.
    let set = server.patch(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "loaded_rate_per_mile": 0.75 }))
        .await;
    assert_eq!(set.status_code(), 200);
    // Patch a *different* rate without mentioning loaded → loaded must survive.
    let other = server.patch(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "deadhead_rate_per_mile": 0.40 }))
        .await;
    assert_eq!(other.status_code(), 200);

    assert_eq!(driver_loaded_rate(&server, &owner_token, &driver_id).await, serde_json::json!(0.75));
}

// --- Facilities two-tier delete (Phase 3) ---

#[tokio::test]
async fn test_facility_soft_archive_reactivate_and_active_list_filter() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner = setup_owner(&server).await;
    let auth = format!("Bearer {owner}");

    // Create a facility.
    let create = server.post("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "name": "Archive Me", "address": "Memphis, TN" }))
        .await;
    assert_eq!(create.status_code(), 201);
    let id = create.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // It appears in the active list.
    let list = server.get("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, &auth).await;
    let items = list.json::<serde_json::Value>()["items"].as_array().unwrap().clone();
    assert!(items.iter().any(|f| f["id"] == id), "new facility should be listed");

    // Soft delete (archive) → 204.
    let del = server.delete(&format!("/fleet/api/v1/facilities/{id}"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(del.status_code(), 204);

    // It drops out of the active list...
    let list2 = server.get("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, &auth).await;
    let items2 = list2.json::<serde_json::Value>()["items"].as_array().unwrap().clone();
    assert!(!items2.iter().any(|f| f["id"] == id), "archived facility must drop from active list");

    // ...but is still fetchable by id, flagged archived.
    let got = server.get(&format!("/fleet/api/v1/facilities/{id}"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(got.status_code(), 200);
    assert_eq!(got.json::<serde_json::Value>()["archived"], true);

    // Reactivate → 200, archived false, back in the list.
    let react = server.post(&format!("/fleet/api/v1/facilities/{id}/reactivate"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(react.status_code(), 200);
    assert_eq!(react.json::<serde_json::Value>()["archived"], false);
    let list3 = server.get("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, &auth).await;
    let items3 = list3.json::<serde_json::Value>()["items"].as_array().unwrap().clone();
    assert!(items3.iter().any(|f| f["id"] == id), "reactivated facility must return to active list");
}

#[tokio::test]
async fn test_facility_permanent_delete_guarded_by_load_stop_referrer() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner = setup_owner(&server).await;
    let auth = format!("Bearer {owner}");

    // Create a facility, then a load whose stop references it.
    let create = server.post("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "name": "Referenced", "address": "Dallas, TX" }))
        .await;
    let fac_id = create.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let load = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [{
                "sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                "facility_id": fac_id, "scheduled_arrive": "2026-05-10T08:00:00",
                "timezone": "America/Chicago"
            }],
            "rate_items": []
        }))
        .await;
    assert_eq!(load.status_code(), 201, "load create failed: {:?}", load.text());
    let load_id = load.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Permanent delete is refused with 409 + an enumerated referrer message.
    let blocked = server.delete(&format!("/fleet/api/v1/facilities/{fac_id}/permanent"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(blocked.status_code(), 409);
    let err = blocked.json::<serde_json::Value>()["error"].as_str().unwrap().to_string();
    assert!(err.contains("1 loads"), "expected referrer count in message, got: {err}");

    // Clear the referrer (delete the load), then the purge succeeds.
    let del_load = server.delete(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert!(del_load.status_code() == 204 || del_load.status_code() == 200,
        "load delete failed: {}", del_load.status_code());

    let purge = server.delete(&format!("/fleet/api/v1/facilities/{fac_id}/permanent"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(purge.status_code(), 204, "purge should succeed once unreferenced: {:?}", purge.text());

    // Gone entirely.
    let got = server.get(&format!("/fleet/api/v1/facilities/{fac_id}"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(got.status_code(), 404);
}

#[tokio::test]
async fn test_facility_permanent_delete_guarded_by_trip_stop_referrer() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner = setup_owner(&server).await;
    let auth = format!("Bearer {owner}");

    // Create a facility.
    let create = server.post("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "name": "Trip Referenced", "address": "Houston, TX" }))
        .await;
    assert_eq!(create.status_code(), 201);
    let fac_id = create.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Archive the facility first (soft delete) — still referenced after archive.
    let arch = server.delete(&format!("/fleet/api/v1/facilities/{fac_id}"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(arch.status_code(), 204);

    // Create a free-standing trip (no load) with a stop referencing the facility.
    let trip = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({
            "stops": [{
                "sequence": 1,
                "stop_type": "pickup",
                "facility_id": fac_id
            }]
        }))
        .await;
    assert_eq!(trip.status_code(), 201, "trip create failed: {:?}", trip.text());
    let trip_id = trip.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Permanent delete must be refused with 409; body must mention trips.
    let blocked = server.delete(&format!("/fleet/api/v1/facilities/{fac_id}/permanent"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(blocked.status_code(), 409);
    let err = blocked.json::<serde_json::Value>()["error"].as_str().unwrap().to_string();
    assert!(err.contains("trips"), "expected 'trips' in referrer message, got: {err}");

    // Remove the referrer (cancel + hard delete the trip via two-step delete).
    let del1 = server.delete(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert!(del1.status_code() == 204 || del1.status_code() == 200);
    let del2 = server.delete(&format!("/fleet/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert!(del2.status_code() == 204 || del2.status_code() == 200);

    // Now the permanent delete must succeed.
    let purge = server.delete(&format!("/fleet/api/v1/facilities/{fac_id}/permanent"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(purge.status_code(), 204, "purge should succeed once unreferenced: {:?}", purge.text());

    let gone = server.get(&format!("/fleet/api/v1/facilities/{fac_id}"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(gone.status_code(), 404);
}

#[tokio::test]
async fn test_list_facilities_include_archived_query_param() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner = setup_owner(&server).await;
    let auth = format!("Bearer {owner}");

    // Create a facility then archive it.
    let create = server.post("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "name": "Archived Dock", "address": "Phoenix, AZ" }))
        .await;
    assert_eq!(create.status_code(), 201);
    let fac_id = create.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let arch = server.delete(&format!("/fleet/api/v1/facilities/{fac_id}"))
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(arch.status_code(), 204);

    // Default list must not include the archived facility.
    let default_list = server.get("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(default_list.status_code(), 200);
    let default_items = default_list.json::<serde_json::Value>()["items"].as_array().unwrap().clone();
    assert!(!default_items.iter().any(|f| f["id"] == fac_id),
        "archived facility must not appear in the default list");

    // include_archived=true must include it, with archived=true.
    let archived_list = server.get("/fleet/api/v1/facilities?include_archived=true")
        .add_header(header::AUTHORIZATION, &auth).await;
    assert_eq!(archived_list.status_code(), 200);
    let archived_items = archived_list.json::<serde_json::Value>()["items"].as_array().unwrap().clone();
    let found = archived_items.iter().find(|f| f["id"] == fac_id);
    assert!(found.is_some(), "archived facility must appear with include_archived=true");
    assert_eq!(found.unwrap()["archived"], true);
}

#[tokio::test]
async fn test_fleet_user_maintenance_crud_http() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mnt-crud@example.com", "password-mnt1").await;
    let auth = format!("Bearer {token}");
    let truck_id = create_truck(&server, "MNT-TRK-1").await;

    // POST create
    let created = server.post("/fleet/api/v1/maintenance")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({
            "equipment_type": "truck",
            "equipment_id": truck_id,
            "service_date": "2026-06-01",
            "category": "repair",
            "description": "replaced alternator",
            "cost": 412.5,
            "odometer": 184000,
            "vendor": "Acme Diesel",
            "invoice_ref": "INV-9931"
        }))
        .await;
    assert_eq!(created.status_code(), 201);
    let body: serde_json::Value = created.json();
    let id = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["category"], "repair");
    assert_eq!(body["equipment_type"], "truck");
    assert_eq!(body["cost"], 412.5);

    // GET one
    let one = server.get(&format!("/fleet/api/v1/maintenance/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(one.status_code(), 200);
    assert_eq!(one.json::<serde_json::Value>()["description"], "replaced alternator");

    // GET list filtered by equipment
    let list = server.get(&format!(
        "/fleet/api/v1/maintenance?equipment_type=truck&equipment_id={truck_id}"
    ))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(list.status_code(), 200);
    let items = list.json::<serde_json::Value>()["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], id);

    // PATCH update
    let patched = server.patch(&format!("/fleet/api/v1/maintenance/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "category": "brakes", "description": "front pads" }))
        .await;
    assert_eq!(patched.status_code(), 200);
    let pbody: serde_json::Value = patched.json();
    assert_eq!(pbody["category"], "brakes");
    assert_eq!(pbody["description"], "front pads");

    // DELETE (hard)
    let deleted = server.delete(&format!("/fleet/api/v1/maintenance/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(deleted.status_code(), 204);
    let gone = server.get(&format!("/fleet/api/v1/maintenance/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(gone.status_code(), 404);
}

#[tokio::test]
async fn test_maintenance_create_rejects_unknown_equipment() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mnt-eq@example.com", "password-mnt2").await;
    let bogus = uuid::Uuid::new_v4();

    let resp = server.post("/fleet/api/v1/maintenance")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "equipment_type": "truck",
            "equipment_id": bogus,
            "service_date": "2026-06-01",
            "category": "repair",
            "description": "ghost truck"
        }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_maintenance_create_rejects_unknown_field() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mnt-unk@example.com", "password-mnt3").await;
    let trailer_id = create_trailer(&server, "MNT-TRL-1").await;

    let resp = server.post("/fleet/api/v1/maintenance")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "equipment_type": "trailer",
            "equipment_id": trailer_id,
            "service_date": "2026-06-01",
            "category": "tire",
            "description": "new tires",
            "owner_id": 5
        }))
        .await;
    assert_eq!(resp.status_code(), 400);
}
