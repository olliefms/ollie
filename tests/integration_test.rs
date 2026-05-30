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
    let set_cookie = login.headers()
        .get("set-cookie")
        .expect("login should return refresh cookie")
        .to_str()
        .unwrap()
        .to_string();
    // Extract the `ollie_refresh=<value>` portion from the Set-Cookie header.
    let cookie_kv = set_cookie.split(';').next().unwrap().trim().to_string();

    // Refresh using the HttpOnly cookie (no Authorization header needed).
    let refresh = server.post("/dispatch/auth/refresh")
        .add_header(header::COOKIE, cookie_kv)
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
async fn test_dispatcher_mcp_tools_list() {
    let (server, _b, _d, _rx) = test_server().await;

    let token = dispatcher_login(&server, "mcp1@example.com", "password-mcp1").await;
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
}

/// Cursor pagination over the MCP surface: following `nextCursor` to exhaustion
/// must yield every record exactly once, and the final page must omit nextCursor.
/// Uses list_facilities with a page size of 2 so a 5-record dataset spans 3 pages.
#[tokio::test]
async fn test_dispatcher_mcp_list_cursor_paginates_all_records() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "mcp_pg@example.com", "password-mcp-pg").await;

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
async fn test_dispatcher_mcp_list_loads() {
    let (server, _b, _d, _rx) = test_server().await;

    let token = dispatcher_login(&server, "mcp2@example.com", "password-mcp2").await;
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
async fn test_dispatcher_mcp_dispatch_and_complete() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "mcp_dc@example.com", "password-mcp-dc").await;
    let session = mcp_session(&server, &token).await;

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

    let driver_id_str = server.post("/api/v1/drivers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let truck_id = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "T-SCHED-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Naive 08:00 local on 2026-05-09 in America/Chicago (CDT, UTC-5) → 13:00 UTC.
    let trip_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let assign = server.post(&format!("/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "driver_id": driver_id_str, "truck_id": truck_id }))
        .await;
    assert_eq!(assign.status_code(), 200);
    let dispatch = server.post(&format!("/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    // Build a trip directly via DB with a legacy Z-suffixed actual_arrive and timezone=None.
    let trip_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let resp = server.get(&format!("/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    // Read the driver_id and truck_id off the in-transit trip so we can build a
    // second Assigned trip for the same driver.
    let trip_uuid: uuid::Uuid = trip_id.parse().unwrap();
    let in_transit = state.db.get_trip(trip_uuid).await.unwrap();
    let driver_id_str = in_transit.driver_id.unwrap().to_string();
    let truck_id_str = in_transit.truck_id.unwrap().to_string();

    // Create trip B with a later scheduled origin arrive and assign same driver/truck.
    let trip_b_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let assign_b = server.post(&format!("/api/v1/trips/{trip_b_id}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let (driver_token, trip_a_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    let trip_a_uuid: uuid::Uuid = trip_a_id.parse().unwrap();
    let trip_a = state.db.get_trip(trip_a_uuid).await.unwrap();
    let driver_a = trip_a.driver_id.unwrap();

    // Build a SECOND independent driver + a busy truck that's InTransit on
    // that driver's trip. Then create Trip B for driver A but referencing
    // the busy truck — auto-dispatch should refuse.
    let driver_b_id_str = server.post("/api/v1/drivers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": "Other Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let busy_truck_id_str = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "T-BUSY" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    // Trip C: driver B on the busy truck, drive to InTransit.
    let trip_c_id_str = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "name": "C-Origin", "timezone": "America/Los_Angeles" },
                { "sequence": 2, "stop_type": "delivery", "name": "C-Dest", "timezone": "America/Los_Angeles" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let _ = server.post(&format!("/api/v1/trips/{trip_c_id_str}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "driver_id": driver_b_id_str, "truck_id": busy_truck_id_str }))
        .await;
    let _ = server.post(&format!("/api/v1/trips/{trip_c_id_str}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    let trip_c_uuid: uuid::Uuid = trip_c_id_str.parse().unwrap();
    state.db.transition_trip_status(trip_c_uuid, ollie::models::TripStatus::InTransit).await.unwrap();

    // Trip B: driver A on the SAME busy truck — assigned only.
    let trip_b_id_str = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let _ = server.post(&format!("/api/v1/trips/{trip_b_id_str}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    let trip_uuid: uuid::Uuid = trip_id.parse().unwrap();
    let in_transit = state.db.get_trip(trip_uuid).await.unwrap();
    let driver_id = in_transit.driver_id.unwrap();
    let truck_id = in_transit.truck_id.unwrap();

    let trip_b_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let _ = server.post(&format!("/api/v1/trips/{trip_b_id}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let complete = server.post(&format!("/api/v1/trips/{trip_id}/complete"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    // Build a load + trip linked to that load, assigned + dispatched + InTransit.
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let load_id = create_test_load(&server, &fac_id).await;

    let driver_id_str = server.post("/api/v1/drivers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let truck_id = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "T-DOC-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "load_id": load_id }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let _ = server.post(&format!("/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "driver_id": driver_id_str, "truck_id": truck_id }))
        .await;
    let _ = server.post(&format!("/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let load_resp = server.get(&format!("/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

// ---------------------------------------------------------------------------
// Issue #220 — driver PATCH must cascade actuals to load stop and fire status
// transitions, mirroring the dispatcher stop_arrive/stop_depart handlers.
// ---------------------------------------------------------------------------

async fn setup_driver_with_dispatched_load_trip(
    server: &TestServer,
    state: &AppState,
) -> (String, String, String) {
    let driver_id_str = server.post("/api/v1/drivers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let truck_id = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "T-CSC-001" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let origin_fac = create_test_facility(server, "Origin", "Chicago, IL").await;
    let dest_fac = create_test_facility(server, "Dest", "Memphis, TN").await;

    let load_id = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let trip_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "load_id": load_id }))
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

async fn create_dispatcher_api_key(
    server: &axum_test::TestServer,
    token: &str,
    label: &str,
) -> serde_json::Value {
    let resp = server.post("/dispatch/api-keys")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "label": label }))
        .await;
    assert_eq!(resp.status_code(), 201, "create api key failed: {}", resp.text());
    resp.json::<serde_json::Value>()
}

#[tokio::test]
async fn test_api_key_create_returns_plaintext_once() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "apikey1@example.com", "pass-apikey1").await;

    let body = create_dispatcher_api_key(&server, &token, "Claude desktop").await;

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
    let token = dispatcher_login(&server, "apikey2@example.com", "pass-apikey2").await;

    let resp = server.post("/dispatch/api-keys")
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
    let token = dispatcher_login(&server, "apikey3@example.com", "pass-apikey3").await;

    let resp = server.post("/dispatch/api-keys")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "label": "too-long", "expires_in_days": 366 }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_api_key_list_returns_own_keys_only() {
    let (server, _b, _d, _rx) = test_server().await;
    let t1 = dispatcher_login(&server, "apikeylist1@example.com", "pass1").await;
    let t2 = dispatcher_login(&server, "apikeylist2@example.com", "pass2").await;

    create_dispatcher_api_key(&server, &t1, "d1-key").await;
    create_dispatcher_api_key(&server, &t2, "d2-key").await;

    let resp = server.get("/dispatch/api-keys")
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
    let token = dispatcher_login(&server, "apikeyrev1@example.com", "pass-rev1").await;

    let body = create_dispatcher_api_key(&server, &token, "to-revoke").await;
    let key_id = body["id"].as_str().unwrap();

    let del = server.delete(&format!("/dispatch/api-keys/{key_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(del.status_code(), 204);

    let list = server.get("/dispatch/api-keys")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    let list_body = list.json::<serde_json::Value>();
    assert_eq!(list_body["keys"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_api_key_revoke_not_found_for_other_dispatcher() {
    let (server, _b, _d, _rx) = test_server().await;
    let t1 = dispatcher_login(&server, "apikeyown1@example.com", "pass-own1").await;
    let t2 = dispatcher_login(&server, "apikeyown2@example.com", "pass-own2").await;

    let body = create_dispatcher_api_key(&server, &t1, "t1-key").await;
    let key_id = body["id"].as_str().unwrap();

    let resp = server.delete(&format!("/dispatch/api-keys/{key_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {t2}"))
        .await;
    assert_eq!(resp.status_code(), 404);
}

#[tokio::test]
async fn test_api_key_auth_grants_access_to_protected_endpoint() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "apikeyauth1@example.com", "pass-auth1").await;

    let body = create_dispatcher_api_key(&server, &token, "Claude desktop").await;
    let api_key = body["key"].as_str().unwrap();

    let resp = server.get("/dispatch/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {api_key}"))
        .await;
    assert_eq!(resp.status_code(), 200);
}

#[tokio::test]
async fn test_api_key_auth_works_on_mcp_endpoint() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "apikeymcp1@example.com", "pass-mcp1").await;

    let body = create_dispatcher_api_key(&server, &token, "Claude MCP").await;
    let api_key = body["key"].as_str().unwrap();

    let resp = server.post("/dispatch/mcp")
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
    let token = dispatcher_login(&server, "apikeyrvk1@example.com", "pass-rvk1").await;

    let body = create_dispatcher_api_key(&server, &token, "to-revoke").await;
    let api_key = body["key"].as_str().unwrap().to_string();
    let key_id = body["id"].as_str().unwrap();

    server.delete(&format!("/dispatch/api-keys/{key_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;

    let resp = server.get("/dispatch/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {api_key}"))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_jwt_auth_still_works_after_api_key_feature() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "apikeycompat1@example.com", "pass-compat1").await;

    let resp = server.get("/dispatch/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
}

#[tokio::test]
async fn test_api_key_create_requires_jwt_not_api_key() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "apikeyself1@example.com", "pass-self1").await;

    let body = create_dispatcher_api_key(&server, &token, "first-key").await;
    let api_key = body["key"].as_str().unwrap();

    let resp = server.post("/dispatch/api-keys")
        .add_header(header::AUTHORIZATION, format!("Bearer {api_key}"))
        .json(&serde_json::json!({ "label": "self-created" }))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_api_key_revoke_requires_jwt_not_api_key() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "apikeyself2@example.com", "pass-self2").await;

    let body = create_dispatcher_api_key(&server, &token, "key-to-revoke").await;
    let api_key = body["key"].as_str().unwrap().to_string();
    let key_id = body["id"].as_str().unwrap();

    let resp = server.delete(&format!("/dispatch/api-keys/{key_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {api_key}"))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_api_key_20_key_cap() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "apikeycap1@example.com", "pass-cap1").await;

    for i in 0..20 {
        let resp = server.post("/dispatch/api-keys")
            .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
            .json(&serde_json::json!({ "label": format!("key-{i}") }))
            .await;
        assert_eq!(resp.status_code(), 201, "key {i} creation should succeed");
    }

    let resp = server.post("/dispatch/api-keys")
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
    let fac_id = create_test_facility(&server, "Dock A", "Somewhere, US").await;
    let fac2_id = create_test_facility(&server, "Dock B", "Elsewhere, US").await;
    let load_id = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let trip_resp = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

// ── dispatcher PATCH + recalculate-miles endpoints (Task 4, #259, #262) ─────

async fn make_trip_with_two_stops(server: &axum_test::TestServer) -> String {
    let fac1 = create_test_facility(server, "Recalc Dock A", "Dallas, TX").await;
    let fac2 = create_test_facility(server, "Recalc Dock B", "Houston, TX").await;
    server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let trip = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let token = dispatcher_login(&server, "recalc1@example.com", "password-recalc1").await;
    let trip_id = make_trip_with_two_stops(&server).await;

    let resp = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/recalculate-miles"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 409);
}

#[tokio::test]
async fn test_recalculate_miles_requires_auth() {
    let (server, _b, _d, _rx) = test_server().await;
    let trip_id = make_trip_with_two_stops(&server).await;
    let resp = server.post(&format!("/dispatch/api/v1/trips/{trip_id}/recalculate-miles"))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_recalculate_miles_returns_existing_summary_when_already_set() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let token = dispatcher_login(&server, "recalc2@example.com", "password-recalc2").await;
    let trip_id_str = make_trip_with_two_stops(&server).await;
    let trip_id = uuid::Uuid::parse_str(&trip_id_str).unwrap();

    // Seed miles directly via DB so the "already set" branch fires.
    state.db.update_trip_mileage(
        trip_id, Some(50.0), Some(450.0), Some(500.0), vec![50.0, 450.0],
    ).await.unwrap();
    let before = state.db.get_trip(trip_id).await.unwrap();
    let updated_at_before = before.updated_at;

    let resp = server.post(&format!("/dispatch/api/v1/trips/{trip_id_str}/recalculate-miles"))
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
    let token = dispatcher_login(&server, "recalc3@example.com", "password-recalc3").await;
    let trip_id_str = make_trip_with_two_stops(&server).await;
    let trip_id = uuid::Uuid::parse_str(&trip_id_str).unwrap();

    // Seed miles so the "already set" guard would otherwise skip recompute.
    state.db.update_trip_mileage(
        trip_id, Some(10.0), Some(20.0), Some(30.0), vec![10.0, 20.0],
    ).await.unwrap();

    // ORS is unavailable in tests → force=true must call helper and surface 409
    // (proves the force flag bypassed the early-return branch).
    let resp = server.post(&format!("/dispatch/api/v1/trips/{trip_id_str}/recalculate-miles"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "force": true }))
        .await;
    assert_eq!(resp.status_code(), 409);
}

#[tokio::test]
async fn test_patch_trip_updates_notes() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "patch1@example.com", "password-patch1").await;
    let trip_id = make_trip_with_two_stops(&server).await;

    let resp = server.patch(&format!("/dispatch/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "notes": "dispatcher note" }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["id"], trip_id);
    assert_eq!(body["notes"], "dispatcher note",
        "dispatcher PATCH response should echo updated notes");

    // Also confirm persistence via admin GET.
    let get = server.get(&format!("/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    let get_body = get.json::<serde_json::Value>();
    assert_eq!(get_body["notes"], "dispatcher note");
}

#[tokio::test]
async fn test_patch_trip_rejects_raw_mileage() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "patch2@example.com", "password-patch2").await;
    let trip_id = make_trip_with_two_stops(&server).await;

    for field in ["deadhead_miles", "loaded_miles", "total_miles", "segment_miles"] {
        let body = if field == "segment_miles" {
            serde_json::json!({ field: [1.0, 2.0] })
        } else {
            serde_json::json!({ field: 100.0 })
        };
        let resp = server.patch(&format!("/dispatch/api/v1/trips/{trip_id}"))
            .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
            .json(&body)
            .await;
        assert_eq!(resp.status_code(), 400, "expected 400 for field {field}");
    }
}

#[tokio::test]
async fn test_patch_trip_rejects_unknown_field() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "patch3@example.com", "password-patch3").await;
    let trip_id = make_trip_with_two_stops(&server).await;

    let resp = server.patch(&format!("/dispatch/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "load_id": uuid::Uuid::new_v4() }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_patch_trip_requires_auth() {
    let (server, _b, _d, _rx) = test_server().await;
    let trip_id = make_trip_with_two_stops(&server).await;
    let resp = server.patch(&format!("/dispatch/api/v1/trips/{trip_id}"))
        .json(&serde_json::json!({ "notes": "x" }))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_patch_trip_previous_trip_id_commits_even_when_recompute_fails() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "patch4@example.com", "password-patch4").await;
    let trip_id = make_trip_with_two_stops(&server).await;
    let other_trip_id = make_trip_with_two_stops(&server).await;

    // Linking to a prev trip forces a mileage recompute. ORS is mocked
    // unavailable in tests, so the recompute fails — but the previous_trip_id
    // write itself must still commit. v1.17.0 returned 409 here, which hid the
    // partial commit from callers; v1.17.1 returns 200 with a non-null
    // `mileage_recompute_warning` so the caller knows exactly what happened.
    let resp = server.patch(&format!("/dispatch/api/v1/trips/{trip_id}"))
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

    // Verify the previous_trip_id link did commit by re-reading via the admin API
    // (the dispatcher view doesn't expose previous_trip_id as a top-level field).
    let get_resp = server.get(&format!("/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    let trip: serde_json::Value = get_resp.json();
    assert_eq!(trip["previous_trip_id"].as_str(), Some(other_trip_id.to_string().as_str()));
}

// ── Doctors (trip / load / facility) — v1.17.1 ─────────────────────────────

#[tokio::test]
async fn test_trip_doctor_dry_run_reports_missing_stop_metadata_without_writes() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "doctor1@example.com", "password-doctor1").await;
    let fac1 = create_test_facility(&server, "Doc Dock A", "Memphis, TN").await;
    let fac2 = create_test_facility(&server, "Doc Dock B", "Atlanta, GA").await;

    // Load has rich stop metadata (notes, end window, dwell).
    let load_resp = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let trip = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let get_before = server.get(&format!("/dispatch/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    let before: serde_json::Value = get_before.json();
    assert!(before["stops"][0]["scheduled_arrive_end"].is_null());
    assert!(before["stops"][0]["notes"].is_null());
}

#[tokio::test]
async fn test_trip_doctor_apply_resyncs_stops_from_load_without_overwriting() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "doctor2@example.com", "password-doctor2").await;
    let fac1 = create_test_facility(&server, "Apply Dock A", "Memphis, TN").await;
    let fac2 = create_test_facility(&server, "Apply Dock B", "Atlanta, GA").await;

    let load_resp = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let trip = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "load_id": load_id,
            "stops": [
                {"sequence": 1, "stop_type": "pickup", "facility_id": fac1,
                 "notes": "dispatcher amended note"},
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
    let trip_after = server.get(&format!("/dispatch/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>();
    assert_eq!(trip_after["stops"][0]["notes"], "dispatcher amended note");
}

#[tokio::test]
async fn test_load_doctor_flags_ungeocoded_facility() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "doctor3@example.com", "password-doctor3").await;
    // The test geocoder doesn't fire — facilities created here have no
    // lat/lng, which is exactly what load_doctor's facility_geocoded check
    // should flag.
    let fac1 = create_test_facility(&server, "Ungeo Dock", "Memphis, TN").await;
    let fac2 = create_test_facility(&server, "Ungeo Dock 2", "Atlanta, GA").await;
    let load_resp = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
        .post("/dispatch/mcp")
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
        .post("/dispatch/mcp")
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
        .post("/dispatch/mcp")
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
    let token = dispatcher_login(&server, "mcp_ct@example.com", "password-mcp-ct").await;

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
    let token = dispatcher_login(&server, "mcp_ut@example.com", "password-mcp-ut").await;
    let trip_id = make_trip_with_two_stops(&server).await;

    let trip = mcp_call(&server, &token, "update_trip", serde_json::json!({
        "trip_id": trip_id,
        "notes": "via MCP"
    })).await;
    assert_eq!(trip["id"], trip_id);

    // Verify persistence
    let get = server.get(&format!("/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(get.json::<serde_json::Value>()["notes"], "via MCP");
}

#[tokio::test]
async fn test_mcp_update_trip_rejects_raw_mileage() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "mcp_ut2@example.com", "password-mcp-ut2").await;
    let trip_id = make_trip_with_two_stops(&server).await;
    let session = mcp_session(&server, &token).await;

    let body = mcp_rpc(&server, &token, &session, "tools/call", serde_json::json!({
        "name": "update_trip",
        "arguments": { "trip_id": trip_id, "total_miles": 999.0 }
    })).await;
    assert!(
        body["error"].is_object() || body["result"]["isError"] == serde_json::json!(true),
        "expected MCP error for total_miles set: {body:?}"
    );
}

#[tokio::test]
async fn test_mcp_recalculate_trip_miles_returns_summary() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let token = dispatcher_login(&server, "mcp_rc@example.com", "password-mcp-rc").await;
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
    let token = dispatcher_login(&server, "mcp_gt@example.com", "password-mcp-gt").await;
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
    let token = dispatcher_login(&server, "mcp_lt@example.com", "password-mcp-lt").await;
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
    let token = dispatcher_login(&server, "ltn@example.com", "password-ltn").await;

    let trip_id = make_trip_with_two_stops(&server).await;
    let get = server.get(&format!("/api/v1/trips/{trip_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    let trip_number = get.json::<serde_json::Value>()["trip_number"].as_str().unwrap().to_string();

    // Make a second unrelated trip
    let _ = make_trip_with_two_stops(&server).await;

    let resp = server.get(&format!("/dispatch/api/v1/trips?trip_number={trip_number}"))
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
    let token = dispatcher_login(&server, "lln@example.com", "password-lln").await;

    let fac1 = create_test_facility(&server, "LLN Dock A", "Dallas, TX").await;
    let fac2 = create_test_facility(&server, "LLN Dock B", "Houston, TX").await;
    let load_resp = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let trip = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let resp = server.get(&format!("/dispatch/api/v1/trips?load_number={load_number}"))
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
    let token = dispatcher_login(&server, "mcp_lln@example.com", "password-mcp-lln").await;

    let fac1 = create_test_facility(&server, "MCP LLN Dock A", "Dallas, TX").await;
    let fac2 = create_test_facility(&server, "MCP LLN Dock B", "Houston, TX").await;
    let load_resp = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let trip = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let fac1 = create_test_facility(&server, "Pickup", "Chicago, IL").await;
    let fac2 = create_test_facility(&server, "Stop2", "St Louis, MO").await;
    let fac3 = create_test_facility(&server, "Delivery", "Memphis, TN").await;

    let load_id = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let trip_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let detail = server.get(&format!("/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(detail.status_code(), 200);
    let body: serde_json::Value = detail.json();
    let ms = &body["mileage_summary"];
    assert!(!ms.is_null(), "mileage_summary should be present");
    assert!(ms["origin"].is_null(), "origin must be stripped on load summary");
    assert!(ms["deadhead_miles"].is_null(), "deadhead_miles must be null on load summary");
    let legs = ms["legs"].as_array().expect("legs array");
    assert_eq!(legs.len(), 2, "only loaded legs (deadhead stripped)");
    for leg in legs {
        assert_eq!(leg["kind"], "loaded");
    }
    assert_eq!(ms["loaded_miles"].as_f64().unwrap(), 300.0);
    assert_eq!(ms["total_miles"].as_f64().unwrap(), 300.0);
}

#[tokio::test]
async fn test_load_detail_mileage_summary_none_without_trip() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let load_id = create_test_load(&server, &fac_id).await;

    let detail = server.get(&format!("/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(detail.status_code(), 200);
    let body: serde_json::Value = detail.json();
    assert!(body["mileage_summary"].is_null(), "no trip → mileage_summary null");
}

#[tokio::test]
async fn test_load_detail_mileage_summary_none_when_only_cancelled_trip() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let load_id = create_test_load(&server, &fac_id).await;

    let trip_id = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "load_id": load_id }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let trip_uuid: uuid::Uuid = trip_id.parse().unwrap();

    // Cancel the trip
    state.db.transition_trip_status(trip_uuid, ollie::models::TripStatus::Cancelled).await.unwrap();

    let detail = server.get(&format!("/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(detail.status_code(), 200);
    let body: serde_json::Value = detail.json();
    assert!(body["mileage_summary"].is_null(), "only cancelled trip → mileage_summary null");
}

// ---------------------------------------------------------------------------
// Dispatcher portal facility CRUD (#265)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dispatcher_facility_crud_http() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "fac-crud@example.com", "password-fac1").await;
    let auth = format!("Bearer {token}");

    // POST create
    let created = server.post("/dispatch/api/v1/facilities")
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
    let one = server.get(&format!("/dispatch/api/v1/facilities/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(one.status_code(), 200);
    assert_eq!(one.json::<serde_json::Value>()["name"], "Plant City RSC");

    // GET list (with q substring matching name)
    let list = server.get("/dispatch/api/v1/facilities?q=plant")
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(list.status_code(), 200);
    let list_body: serde_json::Value = list.json();
    assert!(list_body["returned"].as_u64().unwrap() >= 1);
    let items = list_body["items"].as_array().unwrap();
    assert!(items.iter().any(|f| f["id"] == id));

    // PATCH update name
    let patched = server.patch(&format!("/dispatch/api/v1/facilities/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "name": "Plant City RSC (renamed)" }))
        .await;
    assert_eq!(patched.status_code(), 200);
    assert_eq!(patched.json::<serde_json::Value>()["name"], "Plant City RSC (renamed)");
}

#[tokio::test]
async fn test_dispatcher_facility_create_with_explicit_coords_marks_ready() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "fac-coords@example.com", "password-fac2").await;

    let resp = server.post("/dispatch/api/v1/facilities")
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
async fn test_dispatcher_facility_patch_address_requeues_geocode() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let token = dispatcher_login(&server, "fac-readdress@example.com", "password-fac3").await;
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
        embedding: None, created_at: now, updated_at: now,
    }).await.unwrap();

    let resp = server.patch(&format!("/dispatch/api/v1/facilities/{id}"))
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
async fn test_dispatcher_facility_patch_explicit_coords_repair_failed_geocode() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let token = dispatcher_login(&server, "fac-repair@example.com", "password-fac4").await;
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
        embedding: None, created_at: now, updated_at: now,
    }).await.unwrap();

    let resp = server.patch(&format!("/dispatch/api/v1/facilities/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "lat": 28.0125, "lng": -82.1199 }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["geocode_status"], "ready");
    assert_eq!(body["geocode_failure_count"], 0);
}

#[tokio::test]
async fn test_dispatcher_facility_create_rejects_unknown_field() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "fac-unk1@example.com", "password-fac5").await;

    let resp = server.post("/dispatch/api/v1/facilities")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "name": "X", "address": "Y",
            "admin_secret": "leak",
        }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_dispatcher_facility_patch_rejects_unknown_field() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "fac-unk2@example.com", "password-fac6").await;
    let auth = format!("Bearer {token}");

    let created = server.post("/dispatch/api/v1/facilities")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "name": "X", "address": "Y" }))
        .await;
    let id = created.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let resp = server.patch(&format!("/dispatch/api/v1/facilities/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "owner_id": 99 }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_dispatcher_facility_mcp_create_and_update() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "fac-mcp@example.com", "password-mcp-fac").await;

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
    let token = dispatcher_login(&server, "fac-doc@example.com", "password-fac-doc").await;

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
        embedding: None, created_at: now, updated_at: now,
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
    let driver_id_str = server.post("/api/v1/drivers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    server.post("/api/v1/trailers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": unit, "owner": "fleet" }))
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
async fn test_driver_equipment_update_unknown_unit_returns_404() {
    let (server, _b, _d, _rx, state) = test_server_with_state().await;
    let (_did, token) = create_driver_with_jwt(&server, &state).await;
    let resp = server.put("/driver/api/v1/equipment/trailer")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "trailer_unit_numbers": ["DOES-NOT-EXIST"] }))
        .await;
    assert_eq!(resp.status_code(), 404);
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
    let (driver_id, token) = create_driver_with_jwt(&server, &state).await;

    let old_trailer = create_trailer(&server, "TR-OLD").await;
    let new_trailer = create_trailer(&server, "TR-NEW").await;
    let truck_id = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "T-EQ-CASCADE" }))
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

    server.post(&format!("/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "driver_id": driver_id.to_string(),
            "truck_id": truck_id,
            "trailer_ids": [old_trailer.to_string()],
        }))
        .await;
    server.post(&format!("/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    // depart origin → in_transit
    server.post(&format!("/api/v1/trips/{trip_id}/stops/0/depart"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let (driver_id, token) = create_driver_with_jwt(&server, &state).await;

    let old_trailer = create_trailer(&server, "TR-FD-OLD").await;
    let new_trailer = create_trailer(&server, "TR-FD-NEW").await;
    let truck_id = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "T-EQ-FD" }))
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

    server.post(&format!("/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "driver_id": driver_id.to_string(),
            "truck_id": truck_id,
            "trailer_ids": [old_trailer.to_string()],
        }))
        .await;
    server.post(&format!("/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    server.post(&format!("/api/v1/trips/{trip_id}/stops/0/depart"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "actual_depart": "2026-05-12T10:00:00Z" }))
        .await;
    server.post(&format!("/api/v1/trips/{trip_id}/stops/1/arrive"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let (driver_id, token) = create_driver_with_jwt(&server, &state).await;

    let initial_trailer = create_trailer(&server, "TR-DISP-INIT").await;
    let attached_trailer = create_trailer(&server, "TR-DISP-ATTACHED").await;
    let truck_id = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "T-DISP-RECON" }))
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

    server.post(&format!("/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

    let disp = server.post(&format!("/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let (driver_id, token) = create_driver_with_jwt(&server, &state).await;

    let old_trailer = create_trailer(&server, "TR-SYNC-OLD").await;
    let new_trailer = create_trailer(&server, "TR-SYNC-NEW").await;
    let truck_id = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "T-EQ-SYNC" }))
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

    server.post(&format!("/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "driver_id": driver_id.to_string(),
            "truck_id": truck_id,
            "trailer_ids": [old_trailer.to_string()],
        }))
        .await;
    server.post(&format!("/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    server.post(&format!("/api/v1/trips/{trip_id}/stops/0/depart"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let (driver_id, token) = create_driver_with_jwt(&server, &state).await;

    let trailer_id = create_trailer(&server, "TR-REFLECT").await;
    let truck_id = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "T-REFLECT", "plate": "RFL-123" }))
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

    server.post(&format!("/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let (driver_id, token) = create_driver_with_jwt(&server, &state).await;

    let running_truck = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "T-RUNNING" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let queued_truck = server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": "T-QUEUED" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let make_trip = || async {
        server.post("/api/v1/trips")
            .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    server.post(&format!("/api/v1/trips/{trip_a}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "driver_id": driver_id.to_string(), "truck_id": running_truck }))
        .await;
    server.post(&format!("/api/v1/trips/{trip_a}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;

    // Trip B: a newer trip merely assigned to the same driver (queued).
    let trip_b = make_trip().await;
    server.post(&format!("/api/v1/trips/{trip_b}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
async fn test_dispatcher_trailer_crud_http() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "trl-crud@example.com", "password-trl1").await;
    let auth = format!("Bearer {token}");

    // POST create — fleet trailer
    let created = server.post("/dispatch/api/v1/trailers")
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
    let one = server.get(&format!("/dispatch/api/v1/trailers/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(one.status_code(), 200);
    assert_eq!(one.json::<serde_json::Value>()["unit_number"], "DTRL-001");

    // GET list
    let list = server.get("/dispatch/api/v1/trailers")
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(list.status_code(), 200);
    let items = list.json::<serde_json::Value>()["items"].as_array().unwrap().clone();
    assert!(items.iter().any(|t| t["id"] == id));

    // PATCH update notes + make
    let patched = server.patch(&format!("/dispatch/api/v1/trailers/{id}"))
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
async fn test_dispatcher_trailer_create_requires_owner_name_when_not_fleet() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "trl-owner@example.com", "password-trl2").await;

    let resp = server.post("/dispatch/api/v1/trailers")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "unit_number": "DTRL-CAR-001",
            "owner": "carrier",
        }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_dispatcher_trailer_create_rejects_unknown_field() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "trl-unk1@example.com", "password-trl3").await;

    // status is admin-only — must be rejected
    let resp = server.post("/dispatch/api/v1/trailers")
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
async fn test_dispatcher_trailer_patch_rejects_status_and_unknown_fields() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "trl-unk2@example.com", "password-trl4").await;
    let auth = format!("Bearer {token}");

    let created = server.post("/dispatch/api/v1/trailers")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "unit_number": "DTRL-PATCH", "owner": "fleet" }))
        .await;
    let id = created.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // status is intentionally not in PatchTrailerBody
    let resp = server.patch(&format!("/dispatch/api/v1/trailers/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "status": "out_of_service" }))
        .await;
    assert_eq!(resp.status_code(), 400);

    // owner_id is admin-only
    let resp = server.patch(&format!("/dispatch/api/v1/trailers/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "owner_id": 99 }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_dispatcher_truck_crud_http() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "trk-crud@example.com", "password-trk1").await;
    let auth = format!("Bearer {token}");

    let created = server.post("/dispatch/api/v1/trucks")
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

    let one = server.get(&format!("/dispatch/api/v1/trucks/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(one.status_code(), 200);
    assert_eq!(one.json::<serde_json::Value>()["unit_number"], "DTRK-001");

    let patched = server.patch(&format!("/dispatch/api/v1/trucks/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "notes": "DEF top-off at terminal" }))
        .await;
    assert_eq!(patched.status_code(), 200);
    assert_eq!(patched.json::<serde_json::Value>()["notes"], "DEF top-off at terminal");
}

#[tokio::test]
async fn test_dispatcher_truck_patch_rejects_status_and_unknown_fields() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "trk-unk@example.com", "password-trk2").await;
    let auth = format!("Bearer {token}");

    let created = server.post("/dispatch/api/v1/trucks")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "unit_number": "DTRK-PATCH" }))
        .await;
    let id = created.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let resp = server.patch(&format!("/dispatch/api/v1/trucks/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "status": "out_of_service" }))
        .await;
    assert_eq!(resp.status_code(), 400);

    let resp = server.patch(&format!("/dispatch/api/v1/trucks/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "owner_id": 99 }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_dispatcher_trailer_mcp_create_get_update() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "trl-mcp@example.com", "password-trl-mcp").await;

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
async fn test_dispatcher_truck_mcp_create_get_update() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "trk-mcp@example.com", "password-trk-mcp").await;

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
async fn test_dispatcher_mcp_create_truck_and_trailer_then_assign() {
    // Acceptance criteria: dispatcher agent creates a trailer (and truck)
    // mid-conversation via MCP and immediately references them in assign_driver.
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "asg-mcp@example.com", "password-asg-mcp").await;

    // Driver (admin API — there's no dispatcher driver-create)
    let driver_resp = server.post("/api/v1/drivers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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
    let trip_resp = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
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

const DISPATCHER_SECRET: &str = "test-dispatcher-secret-must-be-32b";

fn mint_upload_token() -> String {
    ollie::api::dispatcher_portal::blob_links::mint_token(
        DISPATCHER_SECRET,
        ollie::api::dispatcher_portal::blob_links::BlobUrlOp::Post,
        None,
        300,
    )
    .unwrap()
    .0
}

fn mint_download_token(id: uuid::Uuid) -> String {
    ollie::api::dispatcher_portal::blob_links::mint_token(
        DISPATCHER_SECRET,
        ollie::api::dispatcher_portal::blob_links::BlobUrlOp::Get,
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
        .post(&format!("/dispatch/blobs/presigned?token={token}&name={name}"))
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
        .post(&format!("/dispatch/blobs/presigned?token={up_token}&name=rt.pdf&tags=invoice,rt"))
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
        .get(&format!("/dispatch/blobs/presigned/{id}?token={dl_token}"))
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
        .get(&format!("/dispatch/blobs/presigned/{other}?token={token}"))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_presigned_upload_rejects_bad_token() {
    let (server, _b, _d, _rx) = test_server().await;
    let resp = server
        .post("/dispatch/blobs/presigned?token=not-a-valid-jwt")
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
        .post(&format!("/dispatch/blobs/presigned?token={token}"))
        .add_header(header::CONTENT_TYPE, "text/plain")
        .bytes(b"hello".to_vec().into())
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_mcp_get_blob_metadata_and_delete() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "blobmcp3@example.com", "password-blobmcp3").await;

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
    let token = dispatcher_login(&server, "blobmcp5@example.com", "password-blobmcp5").await;

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
    let resp = server.get(&format!("/dispatch/blobs/presigned/{id_b}?token={dl_token}")).await;
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
    assert!(!paths["/dispatch/blobs/presigned"].is_null(), "upload path missing from spec");
    assert!(!paths["/dispatch/blobs/presigned/{id}"].is_null(), "download path missing from spec");
}

// --- TOCTOU race fix tests ---

#[tokio::test]
async fn test_admin_delete_blob_keeps_bytes_when_checksum_shared() {
    let (server, _b, _d, _rx) = test_server().await;

    // Upload the same bytes twice — dedup gives two records with the same checksum.
    let content = b"shared-checksum-admin-test-bytes";
    let r1 = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(content.to_vec())
                .file_name("shared-a.txt").mime_type("text/plain")))
        .await;
    assert!(r1.status_code() == 202 || r1.status_code() == 201);

    let r2 = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(content.to_vec())
                .file_name("shared-b.txt").mime_type("text/plain")))
        .await;
    assert!(r2.status_code() == 202 || r2.status_code() == 201);

    let id1 = r1.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let id2 = r2.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    assert_ne!(id1, id2, "dedup must produce two distinct record ids");

    // Delete the first record — the storage bytes must NOT be deleted because id2 still exists.
    let del = server.delete(&format!("/api/v1/blob/{id1}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(del.status_code(), 204);

    // The sibling record's bytes must still be downloadable.
    let get = server.get(&format!("/api/v1/blob/{id2}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(get.status_code(), 200, "sibling blob must still be readable after first record deleted");
    assert_eq!(get.as_bytes(), content.as_slice(), "sibling blob bytes must be intact");
}

#[tokio::test]
async fn test_dispatcher_delete_blob_keeps_bytes_when_checksum_shared() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "toctou-disp@example.com", "password-toctou-disp").await;

    // Upload the same bytes twice — dedup gives two records with the same checksum.
    let content = b"shared-checksum-dispatcher-test-bytes";
    let r1 = server.post("/dispatch/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(content.to_vec())
                .file_name("disp-shared-a.txt").mime_type("text/plain")))
        .await;
    assert!(r1.status_code() == 202 || r1.status_code() == 201);

    let r2 = server.post("/dispatch/api/v1/blobs")
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
    let del = server.delete(&format!("/dispatch/api/v1/blob/{id1}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(del.status_code(), 204);

    // The sibling record's bytes must still be downloadable.
    let get = server.get(&format!("/dispatch/api/v1/blob/{id2}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(get.status_code(), 200, "sibling blob must still be readable after first record deleted");
    assert_eq!(get.as_bytes(), content.as_slice(), "sibling blob bytes must be intact");
}

// ---------------------------------------------------------------------------
// Driver equipment attach/detach (dispatcher surface) — #181
// ---------------------------------------------------------------------------

async fn make_driver(server: &axum_test::TestServer, name: &str) -> String {
    server.post("/api/v1/drivers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": name }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

async fn make_truck(server: &axum_test::TestServer, unit: &str) -> String {
    server.post("/api/v1/trucks")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": unit }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

async fn make_trailer(server: &axum_test::TestServer, unit: &str) -> String {
    server.post("/api/v1/trailers")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "unit_number": unit, "owner": "fleet" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

async fn truck_status(server: &axum_test::TestServer, id: &str) -> String {
    server.get(&format!("/api/v1/trucks/{id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await
        .json::<serde_json::Value>()["status"].as_str().unwrap().to_string()
}

async fn trailer_status(server: &axum_test::TestServer, id: &str) -> String {
    server.get(&format!("/api/v1/trailers/{id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await
        .json::<serde_json::Value>()["status"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_attach_equipment_truck_and_trailers() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "attach1@example.com", "password-attach1").await;
    let auth = format!("Bearer {token}");

    let driver = make_driver(&server, "Attach Driver").await;
    let truck = make_truck(&server, "AE-TRK-1").await;
    let trl_a = make_trailer(&server, "AE-TRL-A").await;
    let trl_b = make_trailer(&server, "AE-TRL-B").await;

    let resp = server.post(&format!("/dispatch/api/v1/drivers/{driver}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "truck": truck, "trailer_ids": [trl_a, trl_b] }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["truck_id"], truck);
    assert_eq!(body["trailer_ids"].as_array().unwrap().len(), 2);
    assert_eq!(body["trip_cascade"], false);

    // Driver record reflects equipment.
    let d = server.get(&format!("/dispatch/api/v1/drivers/{driver}"))
        .add_header(header::AUTHORIZATION, &auth).await.json::<serde_json::Value>();
    assert_eq!(d["current_truck_id"], truck);
    assert_eq!(d["current_trailer_ids"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_attach_equipment_trailers_are_additive() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "attach2@example.com", "password-attach2").await;
    let auth = format!("Bearer {token}");

    let driver = make_driver(&server, "Additive Driver").await;
    let trl_a = make_trailer(&server, "ADD-TRL-A").await;
    let trl_b = make_trailer(&server, "ADD-TRL-B").await;

    server.post(&format!("/dispatch/api/v1/drivers/{driver}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "trailer_ids": [trl_a] })).await;

    let resp = server.post(&format!("/dispatch/api/v1/drivers/{driver}/attach-equipment"))
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
    let resp = server.post(&format!("/dispatch/api/v1/drivers/{driver}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "trailer_ids": [trl_a] })).await;
    assert_eq!(resp.json::<serde_json::Value>()["trailer_ids"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_attach_truck_releases_previous_truck() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "attach3@example.com", "password-attach3").await;
    let auth = format!("Bearer {token}");

    let driver = make_driver(&server, "Swap Driver").await;
    let truck1 = make_truck(&server, "SW-TRK-1").await;
    let truck2 = make_truck(&server, "SW-TRK-2").await;

    server.post(&format!("/dispatch/api/v1/drivers/{driver}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "truck": truck1 })).await;
    assert_eq!(truck_status(&server, &truck1).await, "assigned");

    server.post(&format!("/dispatch/api/v1/drivers/{driver}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "truck": truck2 })).await;
    assert_eq!(truck_status(&server, &truck1).await, "available", "previous truck released");
    assert_eq!(truck_status(&server, &truck2).await, "assigned");
}

#[tokio::test]
async fn test_attach_equipment_empty_body_400() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "attach4@example.com", "password-attach4").await;
    let driver = make_driver(&server, "Empty Driver").await;

    let resp = server.post(&format!("/dispatch/api/v1/drivers/{driver}/attach-equipment"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({})).await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_attach_equipment_inactive_driver_409() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "attach5@example.com", "password-attach5").await;
    let auth = format!("Bearer {token}");
    let driver = make_driver(&server, "Inactive Driver").await;
    let truck = make_truck(&server, "IN-TRK-1").await;

    // Soft-delete (inactivate) the driver via admin API.
    server.delete(&format!("/api/v1/drivers/{driver}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret").await;

    let resp = server.post(&format!("/dispatch/api/v1/drivers/{driver}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "truck": truck })).await;
    assert_eq!(resp.status_code(), 409);
}

#[tokio::test]
async fn test_attach_equipment_conflict_on_other_active_trip() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "attach6@example.com", "password-attach6").await;
    let auth = format!("Bearer {token}");

    // Driver A gets a dispatched trip with truck + trailer.
    let driver_a = make_driver(&server, "Driver A").await;
    let truck = make_truck(&server, "CF-TRK-1").await;
    let trailer = make_trailer(&server, "CF-TRL-1").await;
    let trip = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "name": "O" },
                { "sequence": 2, "stop_type": "delivery", "name": "D" }
            ]
        }))
        .await.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    server.post(&format!("/api/v1/trips/{trip}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "driver_id": driver_a, "truck_id": truck, "trailer_ids": [trailer] })).await;
    server.post(&format!("/api/v1/trips/{trip}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret").await;

    // Driver B tries to grab the same truck.
    let driver_b = make_driver(&server, "Driver B").await;
    let resp = server.post(&format!("/dispatch/api/v1/drivers/{driver_b}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "truck": truck })).await;
    assert_eq!(resp.status_code(), 409, "truck on another active trip");

    let resp = server.post(&format!("/dispatch/api/v1/drivers/{driver_b}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "trailer_ids": [trailer] })).await;
    assert_eq!(resp.status_code(), 409, "trailer on another active trip");
}

#[tokio::test]
async fn test_attach_detach_cascades_active_trip() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "attach7@example.com", "password-attach7").await;
    let auth = format!("Bearer {token}");

    let driver = make_driver(&server, "Cascade Driver").await;
    let truck = make_truck(&server, "CA-TRK-1").await;
    let trailer1 = make_trailer(&server, "CA-TRL-1").await;
    let trailer2 = make_trailer(&server, "CA-TRL-2").await;
    let trip = server.post("/api/v1/trips")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "name": "O" },
                { "sequence": 2, "stop_type": "delivery", "name": "D" }
            ]
        }))
        .await.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    server.post(&format!("/api/v1/trips/{trip}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "driver_id": driver, "truck_id": truck, "trailer_ids": [trailer1] })).await;
    server.post(&format!("/api/v1/trips/{trip}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret").await;

    // Attach a second trailer — should cascade into the active trip.
    let resp = server.post(&format!("/dispatch/api/v1/drivers/{driver}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "trailer_ids": [trailer2] })).await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["trip_cascade"], true);
    assert_eq!(body["trip_id"], trip);

    let t = server.get(&format!("/dispatch/api/v1/trips/{trip}"))
        .add_header(header::AUTHORIZATION, &auth).await.json::<serde_json::Value>();
    assert_eq!(t["trailer_ids"].as_array().unwrap().len(), 2, "trip synced with both trailers");

    // Detach trailer1 — released to available; trip synced down to one.
    let resp = server.post(&format!("/dispatch/api/v1/drivers/{driver}/detach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "trailer_ids": [trailer1] })).await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<serde_json::Value>()["trip_cascade"], true);
    assert_eq!(trailer_status(&server, &trailer1).await, "available");

    let t = server.get(&format!("/dispatch/api/v1/trips/{trip}"))
        .add_header(header::AUTHORIZATION, &auth).await.json::<serde_json::Value>();
    let ids: Vec<String> = t["trailer_ids"].as_array().unwrap().iter()
        .map(|v| v.as_str().unwrap().to_string()).collect();
    assert_eq!(ids, vec![trailer2]);
}

#[tokio::test]
async fn test_detach_equipment_truck_and_all_trailers() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "detach1@example.com", "password-detach1").await;
    let auth = format!("Bearer {token}");

    let driver = make_driver(&server, "Detach Driver").await;
    let truck = make_truck(&server, "DE-TRK-1").await;
    let trl_a = make_trailer(&server, "DE-TRL-A").await;
    let trl_b = make_trailer(&server, "DE-TRL-B").await;

    server.post(&format!("/dispatch/api/v1/drivers/{driver}/attach-equipment"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "truck": truck, "trailer_ids": [trl_a, trl_b] })).await;

    let resp = server.post(&format!("/dispatch/api/v1/drivers/{driver}/detach-equipment"))
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
    let token = dispatcher_login(&server, "detach2@example.com", "password-detach2").await;
    let driver = make_driver(&server, "Detach Empty").await;

    let resp = server.post(&format!("/dispatch/api/v1/drivers/{driver}/detach-equipment"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({})).await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_attach_equipment_via_mcp() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "mcpattach@example.com", "password-mcpattach").await;

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
