// tests/administrative_loads_test.rs
//
// Administrative (no-trip) loads: revenue with no truck behind it — a weekly
// revenue guarantee, TONU, detention-only billing, layover, an accessorial-only
// freight bill. Such a load has no path to `delivered`, because every route
// there is a trip-side cascade, so it invoices straight from `planned`.
//
// These tests guard the isolation that makes that safe: an administrative load
// must never acquire a trip, and its kind must not be changeable once it has
// moved past `planned`.

use axum::http::header;
use axum_test::TestServer;
use ollie::{ai::OllamaClient, api, config::Config, db::DbClient, storage::BlobStore, AppState};
use std::sync::Arc;
use tempfile::TempDir;
use webauthn_rs::prelude::{Url, WebauthnBuilder};

async fn setup() -> (TestServer, AppState, TempDir, TempDir, async_channel::Receiver<ollie::pipeline::PipelineJob>) {
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
    // Keep capacity generous and the receiver alive: dropping it closes the
    // channel and blob uploads (which await pipeline_tx.send) start failing.
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
    (server, state, blob_dir, db_dir, rx)
}

const OWNER_EMAIL: &str = "owner@example.com";
const OWNER_PASSWORD: &str = "owner-password-123";

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

/// Create a stopless load through the API and return its id.
async fn create_bare_load(
    server: &TestServer, token: &str, load_number: &str, kind: &str,
) -> String {
    let resp = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "load_number": load_number,
            "customer_name": "Landstar",
            "stops": [],
            "kind": kind,
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "create failed: {}", resp.text());
    resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_trip_cannot_be_created_against_an_administrative_load() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let load_id = create_bare_load(&server, &token, "4581461", "administrative").await;

    let resp = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "load_id": load_id, "stops": [] }))
        .await;

    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    assert!(
        resp.text().contains("administrative"),
        "the error should name the kind so the caller knows why: {}",
        resp.text(),
    );
}

#[tokio::test]
async fn test_kind_cannot_change_once_the_load_has_left_planned() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let load_id = create_bare_load(&server, &token, "4581461", "administrative").await;

    // planned -> invoiced, an edge only an administrative load has.
    let resp = server.post(&format!("/fleet/api/v1/loads/{load_id}/invoice"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "invoice_number": "JQL-4581461" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Reclassifying it now would leave a status the freight machine can't explain.
    let resp = server.put(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "kind": "freight" }))
        .await;
    assert_eq!(resp.status_code(), 409, "body: {}", resp.text());
}
