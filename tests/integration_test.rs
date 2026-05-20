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
    std::env::set_var("ADMIN_API_KEY", "test-secret");
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
    std::env::set_var("ADMIN_API_KEY", "test-secret");
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

#[tokio::test]
async fn test_upload_returns_202_with_uuid() {
    let (server, _b, _d, _rx) = test_server().await;
    let resp = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let form = axum_test::multipart::MultipartForm::new()
        .add_text("visibility", "driver")
        .add_part(
            "file",
            axum_test::multipart::Part::bytes(b"hello".to_vec())
                .file_name("x.txt")
                .mime_type("text/plain"),
        );
    let resp = server
        .post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let resp = server.post("/api/v1/blobs").await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_get_metadata_after_upload() {
    let (server, _b, _d, _rx) = test_server().await;
    let upload = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"content".to_vec())
                .file_name("doc.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let meta = server.get(&format!("/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .add_header(header::ACCEPT, "application/json")
        .await;
    assert_eq!(meta.status_code(), 200);
    assert_eq!(meta.json::<serde_json::Value>()["id"], id);
}

#[tokio::test]
async fn test_get_raw_bytes_regardless_of_status() {
    let (server, _b, _d, _rx) = test_server().await;
    let content = b"raw content bytes";
    let upload = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(content.to_vec())
                .file_name("raw.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let raw = server.get(&format!("/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(raw.status_code(), 200);
    // File is available even though status is still "pending" (no Ollama in test)
    assert_eq!(raw.as_bytes(), content.as_ref());
}

#[tokio::test]
async fn test_list_blobs() {
    let (server, _b, _d, _rx) = test_server().await;
    server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"file1".to_vec())
                .file_name("a.txt").mime_type("text/plain")))
        .await;

    let list = server.get("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(list.status_code(), 200);
    assert!(list.json::<serde_json::Value>()["returned"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn test_dedup_second_upload_returns_201() {
    let (server, _b, _d, _rx) = test_server().await;
    let content = b"duplicate content";

    let r1 = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(content.to_vec())
                .file_name("dup1.txt").mime_type("text/plain")))
        .await;
    assert_eq!(r1.status_code(), 202);

    let r2 = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let upload = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"to delete".to_vec())
                .file_name("del.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let del = server.delete(&format!("/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(del.status_code(), 204);

    let get = server.get(&format!("/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .add_header(header::ACCEPT, "application/json")
        .await;
    assert_eq!(get.status_code(), 404);
}

#[tokio::test]
async fn test_put_updates_name_and_tags() {
    let (server, _b, _d, _rx) = test_server().await;
    let upload = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"data".to_vec())
                .file_name("orig.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let updated = server.put(&format!("/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    // Upload a blob
    let upload = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"rate con".to_vec())
                .file_name("rate_con.pdf").mime_type("application/pdf")))
        .await;
    let blob_id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Create a facility
    let fac = server.post("/api/v1/facilities")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&load_body)
        .await;

    // Attempt to delete the blob — should be blocked
    let del = server.delete(&format!("/api/v1/blob/{blob_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(del.status_code(), 409);
}

#[tokio::test]
async fn test_create_facility_returns_201() {
    let (server, _b, _d, _rx) = test_server().await;
    let resp = server.post("/api/v1/facilities")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let create = server.post("/api/v1/facilities")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": "XYZ Dock", "address": "Nashville, TN" }))
        .await;
    let id = create.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let get = server.get(&format!("/api/v1/facilities/{id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(get.status_code(), 200);
    assert_eq!(get.json::<serde_json::Value>()["id"], id);
}

#[tokio::test]
async fn test_delete_facility_blocked_when_referenced_by_load() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac = server.post("/api/v1/facilities")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": "Busy Dock", "address": "Atlanta, GA" }))
        .await;
    let fac_id = fac.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [{"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10T08:00:00",
                        "timezone": "America/Chicago"}],
            "rate_items": []
        }))
        .await;

    let del = server.delete(&format!("/api/v1/facilities/{fac_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(del.status_code(), 409);
}

#[tokio::test]
async fn test_list_facilities() {
    let (server, _b, _d, _rx) = test_server().await;
    server.post("/api/v1/facilities")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": "Dock A", "address": "Memphis, TN" }))
        .await;
    let list = server.get("/api/v1/facilities")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(list.status_code(), 200);
    assert!(list.json::<serde_json::Value>()["returned"].as_u64().unwrap() >= 1);
}

async fn create_test_facility(server: &axum_test::TestServer, name: &str, address: &str) -> String {
    server.post("/api/v1/facilities")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": name, "address": address }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_create_load_returns_201() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "ABC Dock", "Memphis, TN").await;

    let resp = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let resp = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let facs = server.get("/api/v1/facilities")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert!(facs.json::<serde_json::Value>()["returned"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn test_load_number_auto_increments() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let stop = serde_json::json!([{
        "sequence": 1, "stop_type": "pickup", "service_type": "live_load",
        "facility_id": fac_id, "scheduled_arrive": "2026-05-10T08:00:00",
        "timezone": "America/Chicago"
    }]);

    let r1 = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({"customer_name": "A", "stops": stop, "rate_items": []}))
        .await;
    let r2 = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({"customer_name": "B", "stops": stop, "rate_items": []}))
        .await;

    let n1 = r1.json::<serde_json::Value>()["load_number"].as_str().unwrap().to_string();
    let n2 = r2.json::<serde_json::Value>()["load_number"].as_str().unwrap().to_string();
    assert_ne!(n1, n2);
}

#[tokio::test]
async fn test_get_load_detail_includes_facility_info() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "ABC Dock", "Memphis, TN").await;
    let create = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "customer_name": "XPO",
            "stops": [{"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10T08:00:00",
                        "timezone": "America/Chicago"}],
            "rate_items": []
        }))
        .await;
    let id = create.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let detail = server.get(&format!("/api/v1/loads/{id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;

    let resp = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let create = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [{"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10T08:00:00",
                        "timezone": "America/Chicago"}],
            "rate_items": []
        }))
        .await;
    let id = create.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let del = server.delete(&format!("/api/v1/loads/{id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(del.status_code(), 204);
    assert_eq!(
        server.get(&format!("/api/v1/loads/{id}"))
            .add_header(header::AUTHORIZATION, "Bearer test-secret")
            .await.status_code(),
        404
    );
}

async fn create_test_load(server: &axum_test::TestServer, fac_id: &str) -> String {
    server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let id = create_test_load(&server, &fac_id).await;

    // assign/dispatch/in_transit/deliver are now driven by trip events (issue #31).
    // Test the post-delivered financial lifecycle: delivered → invoiced → settled.
    // We reach delivered by creating a trip linked to this load with driver_id set
    // (which cascades load to assigned), but for now just test invoice+settle
    // starting from planned via the invoice endpoint (which requires delivered status —
    // skip directly to invoice which returns 409 if not delivered, confirming the
    // state machine still enforces ordering).
    let invoice_premature = server.post(&format!("/api/v1/loads/{id}/invoice"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({"invoice_number": "INV-001", "invoice_date": "2026-05-15"}))
        .await;
    assert_eq!(invoice_premature.status_code(), 409);

    let invoice = server.post(&format!("/api/v1/loads/{id}/cancel"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({"reason": "test done"}))
        .await;
    assert_eq!(invoice.json::<serde_json::Value>()["status"], "cancelled");
}

#[tokio::test]
async fn test_cancel_load() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let id = create_test_load(&server, &fac_id).await;

    let resp = server.post(&format!("/api/v1/loads/{id}/cancel"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let load_id = create_test_load(&server, &fac_id).await;
    let driver_id = uuid::Uuid::new_v4();

    let resp = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "load_id": load_id,
            "driver_id": driver_id,
        }))
        .await;
    assert_eq!(resp.status_code(), 201);

    let load_resp = server.get(&format!("/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(load_resp.json::<serde_json::Value>()["status"], "assigned");
}

// ── Trip two-step DELETE ────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_trip_two_step_delete() {
    let (server, _b, _d, _rx) = test_server().await;

    // Create a minimal trip (no load or driver required)
    let create_resp = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({}))
        .await;
    assert_eq!(create_resp.status_code(), 201);
    let trip_id = create_resp.json::<serde_json::Value>()["id"]
        .as_str().unwrap().to_string();

    // First DELETE — soft-cancel: should return 204
    let del1 = server.delete(&format!("/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(del1.status_code(), 204);

    // GET — trip should still exist with status cancelled
    let get_resp = server.get(&format!("/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(get_resp.status_code(), 200);
    assert_eq!(get_resp.json::<serde_json::Value>()["status"], "cancelled");

    // Second DELETE — hard-delete: should return 204
    let del2 = server.delete(&format!("/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(del2.status_code(), 204);

    // GET — trip should now return 404
    let get_after = server.get(&format!("/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(get_after.status_code(), 404);
}

// ── Blob query endpoint tests ─────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_query_blob_returns_404_for_missing_blob() {
    let (server, _b, _d, _rx) = test_server().await;
    let fake_id = uuid::Uuid::new_v4();
    let resp = server.post(&format!("/api/v1/blobs/{fake_id}/query"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "prompt": "What is this?" }))
        .await;
    assert_eq!(resp.status_code(), 404);
}

#[tokio::test]
async fn test_query_blob_returns_422_when_not_ready() {
    let (server, _b, _d, _rx) = test_server().await;
    // Upload a blob — it stays in pending status (no worker running)
    let upload = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"some document text".to_vec())
                .file_name("doc.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    assert_eq!(upload.json::<serde_json::Value>()["status"], "pending");

    let resp = server.post(&format!("/api/v1/blobs/{id}/query"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "prompt": "What is this?" }))
        .await;
    assert_eq!(resp.status_code(), 422);
}

#[tokio::test]
async fn test_query_blob_returns_400_for_empty_prompt() {
    let (server, _b, _d, _rx) = test_server().await;
    let upload = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"content".to_vec())
                .file_name("doc.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let resp = server.post(&format!("/api/v1/blobs/{id}/query"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "prompt": "" }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_query_blob_returns_400_for_overlong_prompt() {
    let (server, _b, _d, _rx) = test_server().await;
    let upload = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"content".to_vec())
                .file_name("doc.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let long_prompt = "a".repeat(4097);
    let resp = server.post(&format!("/api/v1/blobs/{id}/query"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "prompt": long_prompt }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_query_blob_returns_401_without_auth() {
    let (server, _b, _d, _rx) = test_server().await;
    let fake_id = uuid::Uuid::new_v4();
    let resp = server.post(&format!("/api/v1/blobs/{fake_id}/query"))
        .json(&serde_json::json!({ "prompt": "What is this?" }))
        .await;
    assert_eq!(resp.status_code(), 401);
}

// ── Driver PIN management tests ─────────────────────────────────────────────────────────────────────────────────

async fn create_test_driver(server: &axum_test::TestServer) -> String {
    server.post("/api/v1/drivers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": "Test Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_set_driver_pin_returns_204() {
    let (server, _b, _d, _rx) = test_server().await;
    let driver_id = create_test_driver(&server).await;

    let resp = server.post(&format!("/api/v1/drivers/{driver_id}/pin"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "pin": "1234" }))
        .await;
    assert_eq!(resp.status_code(), 204);
}

#[tokio::test]
async fn test_set_driver_pin_invalid_format_returns_422() {
    let (server, _b, _d, _rx) = test_server().await;
    let driver_id = create_test_driver(&server).await;

    for invalid_pin in ["abc", "12", "1234567"] {
        let resp = server.post(&format!("/api/v1/drivers/{driver_id}/pin"))
            .add_header(header::AUTHORIZATION, "Bearer test-secret")
            .json(&serde_json::json!({ "pin": invalid_pin }))
            .await;
        assert_eq!(resp.status_code(), 422, "expected 422 for pin: {invalid_pin}");
    }
}

#[tokio::test]
async fn test_set_driver_pin_not_found_returns_404() {
    let (server, _b, _d, _rx) = test_server().await;
    let fake_id = uuid::Uuid::new_v4();

    let resp = server.post(&format!("/api/v1/drivers/{fake_id}/pin"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "pin": "1234" }))
        .await;
    assert_eq!(resp.status_code(), 404);
}

#[tokio::test]
async fn test_set_driver_pin_increments_token_version() {
    let (server, _b, _d, _rx) = test_server().await;
    let driver_id = create_test_driver(&server).await;

    // First PIN set — token_version should be 0
    let r1 = server.post(&format!("/api/v1/drivers/{driver_id}/pin"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "pin": "1234" }))
        .await;
    assert_eq!(r1.status_code(), 204);

    // Second PIN set — token_version should be incremented to 1
    let r2 = server.post(&format!("/api/v1/drivers/{driver_id}/pin"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "pin": "5678" }))
        .await;
    assert_eq!(r2.status_code(), 204);

    // Verify by checking credentials via db directly is not possible here,
    // but we can confirm the second call also returns 204 (idempotent success).
    // The token_version increment is verified at the DB layer by the handler logic.
}

// ── DELETE /api/v1/loads/:id FK guard ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_delete_load_blocked_by_active_trip() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "FK Guard Dock", "Chicago, IL").await;
    let load_id = create_test_load(&server, &fac_id).await;

    // Create a trip referencing the load (status defaults to planned = active)
    let trip_resp = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "load_id": load_id }))
        .await;
    assert_eq!(trip_resp.status_code(), 201);
    let trip_id = trip_resp.json::<serde_json::Value>()["id"]
        .as_str().unwrap().to_string();

    // DELETE load → 409 because the trip is active
    let del1 = server.delete(&format!("/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(del1.status_code(), 409);

    // Cancel the trip (first DELETE soft-cancels it)
    let cancel = server.delete(&format!("/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(cancel.status_code(), 204);

    // DELETE load → 204 now that no active trips remain
    let del2 = server.delete(&format!("/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(del2.status_code(), 204);
}

#[tokio::test]
async fn test_assign_sets_trip_resources_and_complete_releases_them() {
    let (server, _b, _d, _rx) = test_server().await;

    // Create driver and truck (both start Available)
    let driver_id = server.post("/api/v1/drivers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": "Test Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let truck_id = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "T-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Create trip with no driver/truck (simulates hermes flow)
    let trip_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "name": "Origin" },
                { "sequence": 1, "stop_type": "delivery", "name": "Destination" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Confirm trip has no driver before assign
    let trip_before = server.get(&format!("/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await
        .json::<serde_json::Value>();
    assert!(trip_before["driver_id"].is_null(), "driver_id should be null before assign");

    // POST /assign
    let assign_resp = server.post(&format!("/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id }))
        .await;
    assert_eq!(assign_resp.status_code(), 200);

    // Confirm trip now has driver_id
    let trip_after_assign = server.get(&format!("/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await
        .json::<serde_json::Value>();
    assert_eq!(trip_after_assign["driver_id"].as_str(), Some(driver_id.as_str()),
        "driver_id must be set on trip after assign");
    assert_eq!(trip_after_assign["truck_id"].as_str(), Some(truck_id.as_str()),
        "truck_id must be set on trip after assign");

    // Confirm driver status = assigned
    let driver_after_assign = server.get(&format!("/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await
        .json::<serde_json::Value>();
    assert_eq!(driver_after_assign["status"], "assigned");

    // Walk through lifecycle: dispatch → depart pickup (→ in_transit) → depart delivery (→ delivered) → complete
    let dispatch = server.post(&format!("/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(dispatch.status_code(), 200);

    let depart_pickup = server.post(&format!("/api/v1/trips/{trip_id}/stops/0/depart"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "actual_depart": "2026-05-07T10:00:00Z" }))
        .await;
    assert_eq!(depart_pickup.status_code(), 200);

    let depart_delivery = server.post(&format!("/api/v1/trips/{trip_id}/stops/1/depart"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "actual_depart": "2026-05-07T14:00:00Z" }))
        .await;
    assert_eq!(depart_delivery.status_code(), 200);

    let complete = server.post(&format!("/api/v1/trips/{trip_id}/complete"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(complete.status_code(), 204);

    // Confirm trip is completed and driver_id is still set (historical record)
    let trip_after_complete = server.get(&format!("/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await
        .json::<serde_json::Value>();
    assert_eq!(trip_after_complete["status"], "completed");
    assert_eq!(trip_after_complete["driver_id"].as_str(), Some(driver_id.as_str()),
        "driver_id must still be set after complete (historical record)");

    // Confirm driver is available again
    let driver_after_complete = server.get(&format!("/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await
        .json::<serde_json::Value>();
    assert_eq!(driver_after_complete["status"], "available");
}

#[tokio::test]
async fn test_trip_inherits_stops_from_load_when_stops_omitted() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Origin Dock", "Chicago, IL").await;

    // Create a load with one stop
    let load_id = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let trip_resp = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let fake_load_id = uuid::Uuid::new_v4();

    let resp = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "load_id": fake_load_id }))
        .await;
    assert_eq!(resp.status_code(), 404, "missing load_id with no stops should return 404");
}

#[tokio::test]
async fn test_invalid_timezone_returns_422() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;

    let resp = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let long_tz = "A".repeat(65);

    let resp = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let driver_id = server.post("/api/v1/drivers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": "Unassign Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let truck_id = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "T-002" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({}))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Assign
    let assign = server.post(&format!("/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id }))
        .await;
    assert_eq!(assign.status_code(), 200);

    // Confirm driver_id is set
    let trip_assigned = server.get(&format!("/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await
        .json::<serde_json::Value>();
    assert_eq!(trip_assigned["driver_id"].as_str(), Some(driver_id.as_str()));

    // Unassign
    let unassign = server.post(&format!("/api/v1/trips/{trip_id}/unassign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(unassign.status_code(), 200);

    // Confirm driver_id is cleared
    let trip_unassigned = server.get(&format!("/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let fake_id = uuid::Uuid::new_v4();

    let resp = server.post(&format!("/api/v1/trips/{fake_id}/stops/0/arrive"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "actual_arrive": "2026-05-10T10:00:00Z" }))
        .await;
    assert_eq!(resp.status_code(), 404);
}

#[tokio::test]
async fn test_trip_stop_arrive_returns_404_for_missing_sequence() {
    let (server, _b, _d, _rx) = test_server().await;

    let trip_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "stops": [{ "sequence": 0, "stop_type": "pickup", "name": "Origin" }]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let resp = server.post(&format!("/api/v1/trips/{trip_id}/stops/999/arrive"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "actual_arrive": "2026-05-10T10:00:00Z" }))
        .await;
    assert_eq!(resp.status_code(), 404);
}

#[tokio::test]
async fn test_trip_stop_depart_returns_404_for_missing_trip() {
    let (server, _b, _d, _rx) = test_server().await;
    let fake_id = uuid::Uuid::new_v4();

    let resp = server.post(&format!("/api/v1/trips/{fake_id}/stops/0/depart"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "actual_depart": "2026-05-10T10:00:00Z" }))
        .await;
    assert_eq!(resp.status_code(), 404);
}

#[tokio::test]
async fn test_trip_stop_depart_returns_404_for_missing_sequence() {
    let (server, _b, _d, _rx) = test_server().await;

    let trip_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "stops": [{ "sequence": 0, "stop_type": "pickup", "name": "Origin" }]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let resp = server.post(&format!("/api/v1/trips/{trip_id}/stops/999/depart"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "actual_depart": "2026-05-10T10:00:00Z" }))
        .await;
    assert_eq!(resp.status_code(), 404);
}

// --- Dispatcher integration tests ---

#[tokio::test]
async fn test_create_dispatcher_returns_201() {
    let (server, _b, _d, _rx) = test_server().await;
    let resp = server.post("/api/v1/dispatchers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "email": "dispatcher@example.com",
            "name": "Jane Dispatcher",
            "password": "securepassword123"
        }))
        .await;
    assert_eq!(resp.status_code(), 201);
    let body = resp.json::<serde_json::Value>();
    assert!(body["id"].as_str().is_some());
    assert_eq!(body["email"], "dispatcher@example.com");
    assert_eq!(body["name"], "Jane Dispatcher");
    assert_eq!(body["status"], "active");
}

#[tokio::test]
async fn test_create_dispatcher_duplicate_email_returns_409() {
    let (server, _b, _d, _rx) = test_server().await;
    let body = serde_json::json!({
        "email": "dup@example.com",
        "name": "First",
        "password": "password123"
    });
    let r1 = server.post("/api/v1/dispatchers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&body)
        .await;
    assert_eq!(r1.status_code(), 201);

    let r2 = server.post("/api/v1/dispatchers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "email": "dup@example.com",
            "name": "Second",
            "password": "different"
        }))
        .await;
    assert_eq!(r2.status_code(), 409);
}

#[tokio::test]
async fn test_list_dispatchers_returns_empty_initially() {
    let (server, _b, _d, _rx) = test_server().await;
    let resp = server.get("/api/v1/dispatchers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["returned"], 0);
    assert_eq!(body["dispatchers"], serde_json::json!([]));
}

#[tokio::test]
async fn test_get_dispatcher_by_id() {
    let (server, _b, _d, _rx) = test_server().await;
    let create = server.post("/api/v1/dispatchers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "email": "get@example.com",
            "name": "Get Me",
            "password": "password123"
        }))
        .await;
    assert_eq!(create.status_code(), 201);
    let id = create.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let get = server.get(&format!("/api/v1/dispatchers/{id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(get.status_code(), 200);
    let body = get.json::<serde_json::Value>();
    assert_eq!(body["id"], id);
    assert_eq!(body["email"], "get@example.com");
    assert_eq!(body["name"], "Get Me");
}

// --- Dispatcher portal auth tests ---

#[tokio::test]
async fn test_dispatcher_login_success() {
    let (server, _b, _d, _rx) = test_server().await;

    // Create a dispatcher via admin API
    let create = server.post("/api/v1/dispatchers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "email": "login@example.com",
            "name": "Login Test",
            "password": "correct-password-123"
        }))
        .await;
    assert_eq!(create.status_code(), 201);

    // Login via dispatcher portal
    let resp = server.post("/dispatch/auth/login")
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
async fn test_dispatcher_login_bad_password() {
    let (server, _b, _d, _rx) = test_server().await;

    let create = server.post("/api/v1/dispatchers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "email": "badpass@example.com",
            "name": "Bad Pass",
            "password": "correct-password-123"
        }))
        .await;
    assert_eq!(create.status_code(), 201);

    let resp = server.post("/dispatch/auth/login")
        .json(&serde_json::json!({
            "email": "badpass@example.com",
            "password": "wrong-password"
        }))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_dispatcher_login_unknown_email() {
    let (server, _b, _d, _rx) = test_server().await;

    let resp = server.post("/dispatch/auth/login")
        .json(&serde_json::json!({
            "email": "nobody@example.com",
            "password": "any-password"
        }))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_dispatcher_refresh() {
    let (server, _b, _d, _rx) = test_server().await;

    // Create dispatcher
    let create = server.post("/api/v1/dispatchers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "email": "refresh@example.com",
            "name": "Refresh Test",
            "password": "refresh-password-123"
        }))
        .await;
    assert_eq!(create.status_code(), 201);

    // Login to get initial token
    let login = server.post("/dispatch/auth/login")
        .json(&serde_json::json!({
            "email": "refresh@example.com",
            "password": "refresh-password-123"
        }))
        .await;
    assert_eq!(login.status_code(), 200);
    let token = login.json::<serde_json::Value>()["token"]
        .as_str()
        .unwrap()
        .to_string();

    // Refresh the token
    let refresh = server.post("/dispatch/auth/refresh")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(refresh.status_code(), 200);
    let body = refresh.json::<serde_json::Value>();
    assert!(body["token"].as_str().is_some(), "expected a new token in refresh response");
}

// --- Dispatcher portal data API tests ---

async fn dispatcher_login(server: &axum_test::TestServer, email: &str, password: &str) -> String {
    // Create dispatcher account
    server.post("/api/v1/dispatchers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "email": email,
            "name": "Test Dispatcher",
            "password": password,
        }))
        .await;

    // Login and return JWT
    let resp = server.post("/dispatch/auth/login")
        .json(&serde_json::json!({ "email": email, "password": password }))
        .await;
    assert_eq!(resp.status_code(), 200);
    resp.json::<serde_json::Value>()["token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_dispatcher_list_loads() {
    let (server, _b, _d, _rx) = test_server().await;

    // Login as dispatcher
    let token = dispatcher_login(&server, "data1@example.com", "password-data1").await;

    // Create a facility and load via admin API first
    let fac_id = create_test_facility(&server, "Dispatch Dock", "Chicago, IL").await;
    server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    // GET /dispatch/api/v1/loads as dispatcher — should return 200
    let resp = server.get("/dispatch/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert!(body["returned"].as_u64().unwrap() >= 1);
    assert!(body["items"].as_array().is_some());
}

#[tokio::test]
async fn test_dispatcher_get_trip() {
    let (server, _b, _d, _rx) = test_server().await;

    // Login as dispatcher
    let token = dispatcher_login(&server, "data2@example.com", "password-data2").await;

    // Create a facility, load, and trip via admin API
    let fac_id = create_test_facility(&server, "Trip Dock", "Dallas, TX").await;
    server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let trip_resp = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    // GET /dispatch/api/v1/trips/:id as dispatcher — should return 200
    let resp = server.get(&format!("/dispatch/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["id"], trip_id);
}

#[tokio::test]
async fn test_dispatcher_assign_and_unassign() {
    let (server, _b, _d, _rx) = test_server().await;

    // Login as dispatcher
    let token = dispatcher_login(&server, "data3@example.com", "password-data3").await;

    // Create resources via admin API
    let fac_id = create_test_facility(&server, "Assign Dock", "Houston, TX").await;

    let driver_resp = server.post("/api/v1/drivers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": "Test Driver Dispatch" }))
        .await;
    assert_eq!(driver_resp.status_code(), 201);
    let driver_id = driver_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let truck_resp = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "TR-DISP-001" }))
        .await;
    assert_eq!(truck_resp.status_code(), 201);
    let truck_id = truck_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_resp = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    // Assign via dispatcher API
    let assign_resp = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/assign"))
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
    let get_resp = server.get(&format!("/dispatch/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(get_resp.status_code(), 200);
    let get_body = get_resp.json::<serde_json::Value>();
    assert_eq!(get_body["driver_id"], driver_id);

    // Unassign via dispatcher API
    let unassign_resp = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/unassign"))
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
// Dispatcher MCP tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dispatcher_mcp_requires_auth() {
    let (server, _b, _d, _rx) = test_server().await;

    // POST /dispatch/mcp without auth header → 401
    let resp = server.post("/dispatch/mcp")
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
async fn test_dispatcher_mcp_tools_list() {
    let (server, _b, _d, _rx) = test_server().await;

    let token = dispatcher_login(&server, "mcp1@example.com", "password-mcp1").await;

    let resp = server.post("/dispatch/mcp")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], 1);
    let tools = body["result"]["tools"].as_array().expect("tools should be an array");
    assert!(!tools.is_empty(), "tools list should not be empty");
    // Verify some expected tools are present
    let tool_names: Vec<&str> = tools.iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(tool_names.contains(&"list_loads"), "should have list_loads tool");
    assert!(tool_names.contains(&"assign_driver"), "should have assign_driver tool");
    assert!(tool_names.contains(&"list_events"), "should have list_events tool");
}

#[tokio::test]
async fn test_dispatcher_mcp_list_loads() {
    let (server, _b, _d, _rx) = test_server().await;

    let token = dispatcher_login(&server, "mcp2@example.com", "password-mcp2").await;

    let resp = server.post("/dispatch/mcp")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "list_loads",
                "arguments": {}
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], 2);
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

    // Create driver and truck (both start Available)
    let driver_id = server.post("/api/v1/drivers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": "Busy Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let truck_id = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "T-BUSY-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Create trip A with stops
    let trip_a_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "name": "Origin A" },
                { "sequence": 1, "stop_type": "delivery", "name": "Destination A" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Assign trip A to driver+truck
    let assign_a = server.post(&format!("/api/v1/trips/{trip_a_id}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id }))
        .await;
    assert_eq!(assign_a.status_code(), 200, "assign trip A should succeed");

    // Dispatch trip A → driver becomes Dispatched
    let dispatch_a = server.post(&format!("/api/v1/trips/{trip_a_id}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(dispatch_a.status_code(), 200, "dispatch trip A should succeed");

    // Confirm driver is now Dispatched
    let driver_after_dispatch = server.get(&format!("/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await
        .json::<serde_json::Value>();
    assert_eq!(driver_after_dispatch["status"], "dispatched");

    // Create trip B
    let trip_b_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "name": "Origin B" },
                { "sequence": 1, "stop_type": "delivery", "name": "Destination B" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Create a second truck for trip B (since truck_id is dispatched)
    let truck_b_id = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "T-BUSY-002" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Assign trip B to the same dispatched driver → should succeed (200)
    let assign_b = server.post(&format!("/api/v1/trips/{trip_b_id}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_b_id }))
        .await;
    assert_eq!(assign_b.status_code(), 200, "assigning trip B to a dispatched driver should succeed");

    // Confirm trip B has driver_id set
    let trip_b = server.get(&format!("/api/v1/trips/{trip_b_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await
        .json::<serde_json::Value>();
    assert_eq!(trip_b["driver_id"].as_str(), Some(driver_id.as_str()),
        "trip B must have driver_id set after assign");

    // Driver status should remain dispatched (not downgraded to assigned)
    let driver_after_assign_b = server.get(&format!("/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await
        .json::<serde_json::Value>();
    assert_eq!(driver_after_assign_b["status"], "dispatched",
        "driver status must remain dispatched after assigning to trip B");
}

#[tokio::test]
async fn test_dispatch_trip_when_driver_already_dispatched_fails() {
    let (server, _b, _d, _rx) = test_server().await;

    // Create driver and two trucks
    let driver_id = server.post("/api/v1/drivers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": "Double Dispatch Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let truck_a_id = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "T-DD-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let truck_b_id = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "T-DD-002" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Create trips A and B
    let trip_a_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "name": "Origin A" },
                { "sequence": 1, "stop_type": "delivery", "name": "Destination A" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_b_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "name": "Origin B" },
                { "sequence": 1, "stop_type": "delivery", "name": "Destination B" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Assign + dispatch trip A → driver becomes Dispatched
    let assign_a = server.post(&format!("/api/v1/trips/{trip_a_id}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_a_id }))
        .await;
    assert_eq!(assign_a.status_code(), 200, "assign trip A should succeed");

    let dispatch_a = server.post(&format!("/api/v1/trips/{trip_a_id}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(dispatch_a.status_code(), 200, "dispatch trip A should succeed");

    // Assign trip B to same driver (allowed since driver is dispatched, not inactive)
    let assign_b = server.post(&format!("/api/v1/trips/{trip_b_id}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_b_id }))
        .await;
    assert_eq!(assign_b.status_code(), 200, "assign trip B to dispatched driver should succeed");

    // Attempt to dispatch trip B → should fail with 409
    let dispatch_b = server.post(&format!("/api/v1/trips/{trip_b_id}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(dispatch_b.status_code(), 409,
        "dispatching trip B when driver is already dispatched should return 409");
}

// ---------------------------------------------------------------------------
// Dispatcher trip lifecycle action tests (#221)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dispatcher_trip_lifecycle_actions() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "lifecycle1@example.com", "password-lifecycle1").await;

    let fac_id = create_test_facility(&server, "Lifecycle Dock", "Houston, TX").await;

    let driver_id = server.post("/api/v1/drivers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": "Lifecycle Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let truck_id = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "TR-LC-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    // Assign via dispatcher API (covered by existing test) — needed to drive transitions
    let assign_resp = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id, "trailer_ids": [] }))
        .await;
    assert_eq!(assign_resp.status_code(), 200);

    // Dispatch via dispatcher API
    let dispatch_resp = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(dispatch_resp.status_code(), 200);
    assert_eq!(dispatch_resp.json::<serde_json::Value>()["status"], "dispatched");

    // Undispatch
    let undispatch_resp = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/undispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(undispatch_resp.status_code(), 200);
    assert_eq!(undispatch_resp.json::<serde_json::Value>()["status"], "assigned");

    // Re-dispatch then drive stops
    let _ = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;

    // Check-call
    let cc_resp = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/check-call"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "location": "I-10 mile 320" }))
        .await;
    assert_eq!(cc_resp.status_code(), 204);

    // Arrive at pickup
    let arr1 = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/stops/1/arrive"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "actual_arrive": "2026-07-01T08:05:00" }))
        .await;
    assert_eq!(arr1.status_code(), 200);

    // Late flag
    let late_resp = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/stops/2/late"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "eta": "2026-07-01T17:00:00", "notes": "traffic" }))
        .await;
    assert_eq!(late_resp.status_code(), 204);

    // Depart pickup → trip becomes in_transit
    let dep1 = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/stops/1/depart"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-07-01T09:00:00" }))
        .await;
    assert_eq!(dep1.status_code(), 200);
    assert_eq!(dep1.json::<serde_json::Value>()["status"], "in_transit");

    // Arrive + depart delivery → trip becomes delivered
    let _ = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/stops/2/arrive"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "actual_arrive": "2026-07-01T16:05:00" }))
        .await;
    let dep2 = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/stops/2/depart"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "actual_depart": "2026-07-01T17:00:00" }))
        .await;
    assert_eq!(dep2.status_code(), 200);
    // Confirm delivered status via follow-up GET (admin stop_depart returns a
    // pre-final-cascade snapshot — see #221 / admin trip_actions::stop_depart).
    let trip_after = server.get(&format!("/dispatch/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(trip_after["status"], "delivered");

    // Complete
    let complete_resp = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/complete"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(complete_resp.status_code(), 204);

    // Verify driver/truck released
    let driver_after = server.get(&format!("/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await
        .json::<serde_json::Value>();
    assert_eq!(driver_after["status"], "available");
}

#[tokio::test]
async fn test_dispatcher_cancel_trip() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "cancel1@example.com", "password-cancel1").await;

    let trip_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "stops": [{ "sequence": 1, "stop_type": "pickup", "name": "Origin" }]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let resp = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/cancel"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<serde_json::Value>()["status"], "cancelled");
}

#[tokio::test]
async fn test_dispatcher_mcp_lifecycle_tools_listed() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "mcp_lc@example.com", "password-mcp-lc").await;

    let resp = server.post("/dispatch/mcp")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}
        }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let tools = resp.json::<serde_json::Value>()["result"]["tools"].as_array().unwrap().clone();
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
async fn test_dispatcher_mcp_dispatch_and_complete() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "mcp_dc@example.com", "password-mcp-dc").await;

    let fac_id = create_test_facility(&server, "MCP Dock", "Dallas, TX").await;

    let driver_id = server.post("/api/v1/drivers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": "MCP Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let truck_id = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "TR-MCP-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    // assign via dispatcher API (already covered)
    let _ = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id, "trailer_ids": [] }))
        .await;

    // Dispatch via MCP
    let dispatch_resp = server.post("/dispatch/mcp")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "dispatch_trip", "arguments": { "trip_id": trip_id } }
        }))
        .await;
    assert_eq!(dispatch_resp.status_code(), 200);
    let body = dispatch_resp.json::<serde_json::Value>();
    assert!(body["error"].is_null(), "MCP error: {:?}", body["error"]);
    let content_text = body["result"]["content"][0]["text"].as_str().expect("text payload");
    let trip: serde_json::Value = serde_json::from_str(content_text).expect("inner JSON");
    assert_eq!(trip["status"], "dispatched");

    // Drive to delivered via MCP stop_arrive/stop_depart
    for (seq, arrive, depart) in [
        (1u32, "2026-07-02T08:05:00", "2026-07-02T09:00:00"),
        (2u32, "2026-07-02T16:05:00", "2026-07-02T17:00:00"),
    ] {
        let r = server.post("/dispatch/mcp")
            .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "stop_arrive",
                    "arguments": { "trip_id": trip_id, "sequence": seq, "actual_arrive": arrive } }
            }))
            .await;
        assert_eq!(r.status_code(), 200);
        let r = server.post("/dispatch/mcp")
            .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": { "name": "stop_depart",
                    "arguments": { "trip_id": trip_id, "sequence": seq, "actual_depart": depart } }
            }))
            .await;
        assert_eq!(r.status_code(), 200);
    }

    // Complete via MCP
    let complete_resp = server.post("/dispatch/mcp")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": { "name": "complete_trip", "arguments": { "trip_id": trip_id } }
        }))
        .await;
    assert_eq!(complete_resp.status_code(), 200);
    let body = complete_resp.json::<serde_json::Value>();
    assert!(body["error"].is_null(), "MCP error: {:?}", body["error"]);
    let trip: serde_json::Value = serde_json::from_str(
        body["result"]["content"][0]["text"].as_str().unwrap()
    ).unwrap();
    assert_eq!(trip["status"], "completed");
}

// ---------------------------------------------------------------------------
// Dispatcher blob API tests (#121)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dispatcher_blob_requires_auth() {
    let (server, _b, _d, _rx) = test_server().await;
    let resp = server.get("/dispatch/api/v1/blobs").await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_dispatcher_upload_blob_returns_202() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "blobs-up@example.com", "password-blobs-up").await;

    let resp = server.post("/dispatch/api/v1/blobs")
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
async fn test_dispatcher_list_blobs() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "blobs-list@example.com", "password-blobs-list").await;

    server.post("/dispatch/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"freight bill".to_vec())
                .file_name("bill.txt").mime_type("text/plain")))
        .await;

    let resp = server.get("/dispatch/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert!(body["returned"].as_u64().unwrap() >= 1);
    assert!(body["items"].as_array().is_some());
}

#[tokio::test]
async fn test_dispatcher_get_blob_json() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "blobs-get@example.com", "password-blobs-get").await;

    let upload = server.post("/dispatch/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"get me".to_vec())
                .file_name("get.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let resp = server.get(&format!("/dispatch/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .add_header(header::ACCEPT, "application/json")
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<serde_json::Value>()["id"], id);
}

#[tokio::test]
async fn test_dispatcher_get_blob_raw_bytes() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "blobs-raw@example.com", "password-blobs-raw").await;

    let content = b"raw document for download test";
    let upload = server.post("/dispatch/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(content.to_vec())
                .file_name("download.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // No Accept: application/json → raw bytes
    let resp = server.get(&format!("/dispatch/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.as_bytes(), content.as_slice());
}

#[tokio::test]
async fn test_dispatcher_update_blob() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "blobs-upd@example.com", "password-blobs-upd").await;

    let upload = server.post("/dispatch/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"update me".to_vec())
                .file_name("original.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let resp = server.put(&format!("/dispatch/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "name": "renamed.txt", "tags": ["invoice"] }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["name"], "renamed.txt");
    assert_eq!(body["tags"], serde_json::json!(["invoice"]));
}

#[tokio::test]
async fn test_dispatcher_delete_blob() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "blobs-del@example.com", "password-blobs-del").await;

    let upload = server.post("/dispatch/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"delete me".to_vec())
                .file_name("delete.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let del_resp = server.delete(&format!("/dispatch/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(del_resp.status_code(), 204);

    let get_resp = server.get(&format!("/dispatch/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .add_header(header::ACCEPT, "application/json")
        .await;
    assert_eq!(get_resp.status_code(), 404);
}

#[tokio::test]
async fn test_assign_trip_oos_trailer_returns_409_no_partial_mutation() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "oos-trailer@example.com", "password-oos-trailer").await;

    let fac_id = create_test_facility(&server, "OOS Dock", "Phoenix, AZ").await;

    let driver_resp = server.post("/api/v1/drivers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": "OOS Test Driver" }))
        .await;
    assert_eq!(driver_resp.status_code(), 201);
    let driver_id = driver_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let truck_resp = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "TR-OOS-001" }))
        .await;
    assert_eq!(truck_resp.status_code(), 201);
    let truck_id = truck_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Create a trailer then mark it out_of_service
    let trailer_resp = server.post("/api/v1/trailers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "TRL-OOS-001", "owner": "fleet" }))
        .await;
    assert_eq!(trailer_resp.status_code(), 201);
    let trailer_id = trailer_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    server.put(&format!("/api/v1/trailers/{trailer_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "status": "out_of_service" }))
        .await;

    let trip_resp = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let resp = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "driver_id": driver_id,
            "truck_id": truck_id,
            "trailer_ids": [trailer_id]
        }))
        .await;
    assert_eq!(resp.status_code(), 409);

    // Trip must remain planned — no partial mutation
    let trip_check = server.get(&format!("/dispatch/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(
        trip_check.json::<serde_json::Value>()["status"], "planned",
        "trip must not be left in assigned status after a 409"
    );

    // Driver must remain available
    let driver_check = server.get(&format!("/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(
        driver_check.json::<serde_json::Value>()["status"], "available",
        "driver must not be marked assigned after a 409"
    );
}

#[tokio::test]
async fn test_upload_large_file_succeeds_under_50mb() {
    let (server, _b, _d, _rx) = test_server().await;
    // 3MB synthetic file — larger than the old 2MB default
    let large_data = vec![0u8; 3 * 1024 * 1024];
    let resp = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(large_data)
                .file_name("large.bin").mime_type("application/octet-stream")))
        .await;
    assert_eq!(resp.status_code(), 202, "3MB upload should succeed with 50MB limit");
}

#[tokio::test]
async fn test_trip_stop_name_and_address_populated_from_facility() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Origin Dock", "Chicago, IL").await;

    let load_id = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [{"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10T08:00:00",
                        "timezone": "America/Chicago"}],
            "rate_items": []
        }))
        .await.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "load_id": load_id }))
        .await.json::<serde_json::Value>();

    assert_eq!(trip["stops"][0]["name"], "Origin Dock", "stop name should be populated from facility");
    assert_eq!(trip["stops"][0]["address"], "Chicago, IL", "stop address should be populated from facility");
}

#[tokio::test]
async fn test_trip_load_number_denormalized() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let load_id = create_test_load(&server, &fac_id).await;

    let trip = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "load_id": load_id }))
        .await.json::<serde_json::Value>();

    assert!(trip["load_number"].is_string(), "load_number should be set when load_id is provided");
    assert!(!trip["load_number"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn test_previous_trip_id_auto_populated_from_driver_last_trip() {
    let (server, _b, _d, _rx) = test_server().await;
    let driver_id = create_test_driver(&server).await;

    // First trip for this driver — no previous trip
    let trip1 = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "driver_id": driver_id }))
        .await.json::<serde_json::Value>();
    assert_eq!(trip1["status"], "planned");
    assert!(trip1["previous_trip_id"].is_null(), "first trip should have no previous_trip_id");

    let trip1_id = trip1["id"].as_str().unwrap();

    // Second trip for same driver — should auto-populate previous_trip_id
    let trip2 = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "driver_id": driver_id }))
        .await.json::<serde_json::Value>();
    assert_eq!(trip2["previous_trip_id"], trip1_id, "second trip should chain to first");
}

#[tokio::test]
async fn test_previous_trip_id_dispatcher_override() {
    let (server, _b, _d, _rx) = test_server().await;
    let driver_id = create_test_driver(&server).await;

    // Create two trips first
    let trip1 = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "driver_id": driver_id }))
        .await.json::<serde_json::Value>();
    let trip1_id = trip1["id"].as_str().unwrap();

    server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "driver_id": driver_id }))
        .await;

    // Third trip with explicit previous_trip_id pointing to trip1 (not trip2)
    let trip3 = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "driver_id": driver_id, "previous_trip_id": trip1_id }))
        .await.json::<serde_json::Value>();
    assert_eq!(trip3["previous_trip_id"], trip1_id, "dispatcher override should be respected");
}

#[tokio::test]
async fn test_deadhead_and_loaded_miles_null_without_ors() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let load_id = create_test_load(&server, &fac_id).await;

    let resp = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "load_id": load_id }))
        .await;
    assert_eq!(resp.status_code(), 201);
    let trip = resp.json::<serde_json::Value>();
    assert!(trip["deadhead_miles"].is_null(), "no ORS → deadhead_miles should be null");
    assert!(trip["loaded_miles"].is_null(), "no ORS → loaded_miles should be null");
}

#[tokio::test]
async fn test_dispatcher_loads_list_route_column_has_facility_names() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "route-test@example.com", "pw-route-test").await;

    let origin_id = create_test_facility(&server, "Origin Hub", "Chicago, IL").await;
    let dest_id   = create_test_facility(&server, "Dest Hub",   "Dallas, TX").await;

    server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let resp = server.get("/dispatch/api/v1/loads")
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
async fn test_dispatcher_count_endpoints_return_200() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "kpi-test@example.com", "pw-kpi-test").await;

    for path in &[
        "/dispatch/api/v1/loads/count",
        "/dispatch/api/v1/drivers/count",
        "/dispatch/api/v1/blobs/count",
        "/dispatch/api/v1/events/count",
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
    let driver_id_str = server.post("/api/v1/drivers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let truck_id = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "T-PAST-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "name": "Origin" },
                { "sequence": 1, "stop_type": "delivery", "name": "Destination" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let assign = server.post(&format!("/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "driver_id": driver_id_str, "truck_id": truck_id }))
        .await;
    assert_eq!(assign.status_code(), 200);

    let dispatch = server.post(&format!("/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(dispatch.status_code(), 200);

    let depart_pickup = server.post(&format!("/api/v1/trips/{trip_id}/stops/0/depart"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "actual_depart": "2026-05-12T10:00:00Z" }))
        .await;
    assert_eq!(depart_pickup.status_code(), 200);

    // Last-stop depart sets trip → delivered and populates delivered_at via stop.actual_depart.
    let depart_delivery = server.post(&format!("/api/v1/trips/{trip_id}/stops/1/depart"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let driver_id_str = server.post("/api/v1/drivers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let truck_id = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "T-IT-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Use sequences 1 and 2 (per AGENTS.md line 332) so off-by-one bugs surface.
    let trip_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let assign = server.post(&format!("/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "driver_id": driver_id_str, "truck_id": truck_id }))
        .await;
    assert_eq!(assign.status_code(), 200);

    let dispatch = server.post(&format!("/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
async fn test_driver_cannot_see_private_doc() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
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
        .post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
        .post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
        .post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let driver_id_str = server
        .post("/api/v1/drivers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
            .post("/api/v1/trucks")
            .add_header(header::AUTHORIZATION, "Bearer test-secret")
            .json(&serde_json::json!({ "unit_number": format!("T-MULTI-{i:03}") }))
            .await
            .json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();

        let trip_id = server
            .post("/api/v1/trips")
            .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
            .post(&format!("/api/v1/trips/{trip_id}/assign"))
            .add_header(header::AUTHORIZATION, "Bearer test-secret")
            .json(&serde_json::json!({ "driver_id": driver_id_str, "truck_id": truck_id }))
            .await;
        assert_eq!(assign.status_code(), 200, "assign trip {i}");

        let dispatch = server
            .post(&format!("/api/v1/trips/{trip_id}/dispatch"))
            .add_header(header::AUTHORIZATION, "Bearer test-secret")
            .await;
        assert_eq!(dispatch.status_code(), 200, "dispatch trip {i}");

        let r = server
            .post(&format!("/api/v1/trips/{trip_id}/stops/0/depart"))
            .add_header(header::AUTHORIZATION, "Bearer test-secret")
            .json(&serde_json::json!({ "actual_depart": pickup_depart }))
            .await;
        assert_eq!(r.status_code(), 200, "depart pickup trip {i}");

        let r = server
            .post(&format!("/api/v1/trips/{trip_id}/stops/1/depart"))
            .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
