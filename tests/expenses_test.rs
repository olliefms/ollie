// tests/expenses_test.rs
//
// Integration tests for fleet REST expense create/list/get (#233 task 5).

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

const OWNER_EMAIL: &str = "owner@example.com";
const OWNER_PASSWORD: &str = "owner-password-123";

/// First-run owner bootstrap (idempotent), returning an owner JWT.
async fn setup_owner(server: &TestServer) -> String {
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

async fn create_expense(server: &TestServer, token: &str, category: &str) -> String {
    let resp = server.post("/fleet/api/v1/expenses")
        .authorization_bearer(token)
        .json(&serde_json::json!({ "category": category }))
        .await;
    assert_eq!(resp.status_code(), 201);
    resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_create_and_get_expense() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    let resp = server.post("/fleet/api/v1/expenses")
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "category": "fuel" }))
        .await;
    assert_eq!(resp.status_code(), 201);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "submitted");
    assert_eq!(body["category"], "fuel");
    assert!(body["amount"].is_null());
    assert!(body["submitted_by"].as_str().unwrap().starts_with("fleet_user:"));
    let id = body["id"].as_str().unwrap();

    let got = server.get(&format!("/fleet/api/v1/expenses/{id}"))
        .authorization_bearer(&token).await;
    assert_eq!(got.status_code(), 200);
    assert_eq!(got.json::<serde_json::Value>()["id"], *id);
}

#[tokio::test]
async fn test_create_expense_validates_links() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    // unknown driver -> 400
    let resp = server.post("/fleet/api/v1/expenses")
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "category": "repair",
            "driver_id": uuid::Uuid::new_v4(),
        })).await;
    assert_eq!(resp.status_code(), 400);
    // equipment_type without equipment_id -> 400
    let resp = server.post("/fleet/api/v1/expenses")
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "category": "repair", "equipment_type": "truck" }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_list_expenses_filters() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    for cat in ["fuel", "tolls"] {
        let r = server.post("/fleet/api/v1/expenses")
            .authorization_bearer(&token)
            .json(&serde_json::json!({ "category": cat })).await;
        assert_eq!(r.status_code(), 201);
    }
    let all = server.get("/fleet/api/v1/expenses").authorization_bearer(&token).await;
    assert_eq!(all.status_code(), 200);
    assert_eq!(all.json::<serde_json::Value>()["total"], 2);
    let fuel = server.get("/fleet/api/v1/expenses?category=fuel")
        .authorization_bearer(&token).await;
    assert_eq!(fuel.json::<serde_json::Value>()["total"], 1);
    let queue = server.get("/fleet/api/v1/expenses?status=submitted")
        .authorization_bearer(&token).await;
    assert_eq!(queue.json::<serde_json::Value>()["total"], 2);
}

#[tokio::test]
async fn test_expenses_require_auth() {
    let (server, _d1, _d2, _rx) = test_server().await;
    assert_eq!(server.get("/fleet/api/v1/expenses").await.status_code(), 401);
}

#[tokio::test]
async fn test_create_expense_rejects_negative_amount() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    let resp = server.post("/fleet/api/v1/expenses")
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "category": "fuel", "amount": -5.0 }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_get_expense_not_found() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    let resp = server.get(&format!("/fleet/api/v1/expenses/{}", uuid::Uuid::new_v4()))
        .authorization_bearer(&token).await;
    assert_eq!(resp.status_code(), 404);
}

#[tokio::test]
async fn test_create_expense_rejects_unknown_category() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    let resp = server.post("/fleet/api/v1/expenses")
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "category": "not-a-real-category" }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_list_expenses_rejects_unknown_status_and_category() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    let _ = create_expense(&server, &token, "fuel").await;
    let resp = server.get("/fleet/api/v1/expenses?status=not-a-status")
        .authorization_bearer(&token).await;
    assert_eq!(resp.status_code(), 400);
    let resp = server.get("/fleet/api/v1/expenses?category=not-a-category")
        .authorization_bearer(&token).await;
    assert_eq!(resp.status_code(), 400);
}
