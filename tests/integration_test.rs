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

async fn test_server() -> (TestServer, TempDir, TempDir, async_channel::Receiver<uuid::Uuid>) {
    let blob_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    std::env::set_var("ADMIN_API_KEY", "test-secret");

    let config = Arc::new(Config::from_env().unwrap());
    let db = Arc::new(DbClient::new(db_dir.path().to_str().unwrap(), 4).await.unwrap());
    let store = Arc::new(BlobStore::new(blob_dir.path().to_str().unwrap()));
    let ai = Arc::new(OllamaClient::new(
        "http://localhost:11434", "nomic-embed-text", "llama3.2", "llava",
    ));
    let geocoding = Arc::new(ollie::geocoding::GeocodingClient::new());
    let ors = Arc::new(ollie::routing::RoutingClient::new(""));

    let (pipeline_tx, rx) = async_channel::bounded(100);
    let (geocoding_tx, _grx) = async_channel::bounded(100);
    let (routing_tx, _rrx) = async_channel::bounded(100);

    let state = AppState {
        db, store, ai, geocoding, ors,
        pipeline_tx, geocoding_tx, routing_tx, config,
    };
    let server = TestServer::new(api::router(state)).unwrap();
    (server, blob_dir, db_dir, rx)
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
    assert!(list.json::<serde_json::Value>()["total"].as_u64().unwrap() >= 1);
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
            "facility_id": fac_id, "scheduled_arrive": "2026-05-10"
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
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10"}],
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
    assert!(list.json::<serde_json::Value>()["total"].as_u64().unwrap() >= 1);
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
                "facility_id": fac_id, "scheduled_arrive": "2026-05-10"
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
                "scheduled_arrive": "2026-05-10"
            }],
            "rate_items": []
        }))
        .await;
    assert_eq!(resp.status_code(), 201);

    let facs = server.get("/api/v1/facilities")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert!(facs.json::<serde_json::Value>()["total"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn test_load_number_auto_increments() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let stop = serde_json::json!([{
        "sequence": 1, "stop_type": "pickup", "service_type": "live_load",
        "facility_id": fac_id, "scheduled_arrive": "2026-05-10"
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
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10"}],
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
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10"}],
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
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10"}],
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
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10"}],
            "rate_items": [{"description": "Line Haul", "amount_usd": 1500.0}]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_dispatch_transitions_to_dispatched() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let id = create_test_load(&server, &fac_id).await;

    let resp = server.post(&format!("/api/v1/loads/{id}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<serde_json::Value>()["status"], "dispatched");
}

#[tokio::test]
async fn test_invalid_transition_returns_409() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let id = create_test_load(&server, &fac_id).await;

    let resp = server.post(&format!("/api/v1/loads/{id}/deliver"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(resp.status_code(), 409);
}

#[tokio::test]
async fn test_full_load_lifecycle() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let id = create_test_load(&server, &fac_id).await;

    let dispatch = server.post(&format!("/api/v1/loads/{id}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret").await;
    assert_eq!(dispatch.json::<serde_json::Value>()["status"], "dispatched");

    let in_transit = server.post(&format!("/api/v1/loads/{id}/in_transit"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret").await;
    assert_eq!(in_transit.json::<serde_json::Value>()["status"], "in_transit");

    let deliver = server.post(&format!("/api/v1/loads/{id}/deliver"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret").await;
    assert_eq!(deliver.json::<serde_json::Value>()["status"], "delivered");

    let invoice = server.post(&format!("/api/v1/loads/{id}/invoice"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({"invoice_number": "INV-001", "invoice_date": "2026-05-15"}))
        .await;
    let body = invoice.json::<serde_json::Value>();
    assert_eq!(body["status"], "invoiced");
    assert_eq!(body["invoice_number"], "INV-001");

    let settle = server.post(&format!("/api/v1/loads/{id}/settle"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret").await;
    assert_eq!(settle.json::<serde_json::Value>()["status"], "settled");
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
async fn test_assign_transitions_planned_to_dispatched() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let id = create_test_load(&server, &fac_id).await;

    let resp = server.post(&format!("/api/v1/loads/{id}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<serde_json::Value>()["status"], "dispatched");
}
