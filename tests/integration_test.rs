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
