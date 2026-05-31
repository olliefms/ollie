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
    let server = TestServer::new(api::router(state)).unwrap();
    (server, blob_dir, db_dir)
}

/// Create a dispatcher account and log in, returning a JWT.
async fn dispatcher_login(server: &TestServer, email: &str, password: &str) -> String {
    // Create dispatcher via admin API (ignore 409 if already exists)
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
