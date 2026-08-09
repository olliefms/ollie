// tests/it/fleet_pagination_test.rs
//
// Regression coverage for real limit/offset pagination on the fleet REST list
// endpoints that previously hard-coded `list_x(..., 100, 0)` regardless of the
// query string, plus the new `total` fields added alongside pagination so the
// fleet UI can build a pager without depending on page size == total count.

use axum::http::header;
use axum_test::TestServer;
use ollie::{ai::OllamaClient, api, config::Config, db::DbClient, storage::BlobStore, AppState};
use std::collections::HashSet;
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

// ---------------------------------------------------------------------------
// Drivers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_list_drivers_pagination_windows_and_total() {
    let (server, _state, _b, _d, _rx) = setup().await;
    let owner_token = setup_owner(&server).await;

    for i in 0..5 {
        let resp = server.post("/fleet/api/v1/drivers")
            .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
            .json(&serde_json::json!({ "name": format!("Driver {i}") }))
            .await;
        assert_eq!(resp.status_code(), 201);
    }

    let page1 = server.get("/fleet/api/v1/drivers?limit=2&offset=0")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(page1.status_code(), 200);
    let page1_body = page1.json::<serde_json::Value>();
    assert_eq!(page1_body["items"].as_array().unwrap().len(), 2, "page1 must return exactly `limit` items");
    assert_eq!(page1_body["returned"], 5, "returned carries the total count for backward compat");

    let page2 = server.get("/fleet/api/v1/drivers?limit=2&offset=2")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    let page2_body = page2.json::<serde_json::Value>();
    assert_eq!(page2_body["items"].as_array().unwrap().len(), 2);

    let page3 = server.get("/fleet/api/v1/drivers?limit=2&offset=4")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    let page3_body = page3.json::<serde_json::Value>();
    assert_eq!(page3_body["items"].as_array().unwrap().len(), 1, "last page holds the remainder (5 - 4)");

    // Windows must not overlap and must cover every seeded driver exactly once.
    let mut ids: HashSet<String> = HashSet::new();
    for body in [&page1_body, &page2_body, &page3_body] {
        for item in body["items"].as_array().unwrap() {
            let id = item["id"].as_str().unwrap().to_string();
            assert!(ids.insert(id), "driver appeared in more than one page — windows overlap");
        }
    }
    assert_eq!(ids.len(), 5);
}

#[tokio::test]
async fn test_list_drivers_default_limit_is_used_when_omitted() {
    let (server, _state, _b, _d, _rx) = setup().await;
    let owner_token = setup_owner(&server).await;

    for i in 0..3 {
        server.post("/fleet/api/v1/drivers")
            .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
            .json(&serde_json::json!({ "name": format!("Driver {i}") }))
            .await;
    }

    let resp = server.get("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["items"].as_array().unwrap().len(), 3);
    assert_eq!(body["returned"], 3);
}

// ---------------------------------------------------------------------------
// Blobs
// ---------------------------------------------------------------------------

async fn upload_blob(server: &TestServer, owner_token: &str, content: &[u8], name: &str) {
    let form = axum_test::multipart::MultipartForm::new()
        .add_text("name", name)
        .add_part(
            "file",
            axum_test::multipart::Part::bytes(content.to_vec())
                .file_name(name)
                .mime_type("text/plain"),
        );
    let resp = server.post("/fleet/api/v1/blobs")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .multipart(form)
        .await;
    let sc = resp.status_code().as_u16();
    assert!(sc == 201 || sc == 202, "blob upload failed with {sc}");
}

#[tokio::test]
async fn test_list_blobs_pagination_windows_and_total() {
    let (server, _state, _b, _d, _rx) = setup().await;
    let owner_token = setup_owner(&server).await;

    for i in 0..5 {
        upload_blob(&server, &owner_token, format!("blob body {i}").as_bytes(), &format!("doc-{i}.txt")).await;
    }

    let page1 = server.get("/fleet/api/v1/blobs?limit=2&offset=0")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(page1.status_code(), 200);
    let page1_body = page1.json::<serde_json::Value>();
    assert_eq!(page1_body["items"].as_array().unwrap().len(), 2);
    assert_eq!(page1_body["total"], 5, "new `total` field must carry the full matching count");
    assert_eq!(page1_body["returned"], 2, "`returned` is the count on this page");

    let page3 = server.get("/fleet/api/v1/blobs?limit=2&offset=4")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    let page3_body = page3.json::<serde_json::Value>();
    assert_eq!(page3_body["items"].as_array().unwrap().len(), 1);
    assert_eq!(page3_body["total"], 5);

    let page2 = server.get("/fleet/api/v1/blobs?limit=2&offset=2")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    let page2_body = page2.json::<serde_json::Value>();

    let mut ids: HashSet<String> = HashSet::new();
    for body in [&page1_body, &page2_body, &page3_body] {
        for item in body["items"].as_array().unwrap() {
            let id = item["id"].as_str().unwrap().to_string();
            assert!(ids.insert(id), "blob appeared in more than one page — windows overlap");
        }
    }
    assert_eq!(ids.len(), 5);
}

// ---------------------------------------------------------------------------
// Trucks / Trailers — cheap smoke coverage that limit/offset are threaded
// through (same root-cause bug as drivers, no dedicated `total` field to lock
// since TruckListResponse/TrailerListResponse live outside this change's
// file-ownership boundary — see PR notes).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_list_trucks_pagination_windows() {
    let (server, _state, _b, _d, _rx) = setup().await;
    let owner_token = setup_owner(&server).await;

    for i in 0..5 {
        server.post("/fleet/api/v1/trucks")
            .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
            .json(&serde_json::json!({ "unit_number": format!("T-PG-{i}") }))
            .await;
    }

    let page1 = server.get("/fleet/api/v1/trucks?limit=2&offset=0")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    let page1_body = page1.json::<serde_json::Value>();
    assert_eq!(page1_body["items"].as_array().unwrap().len(), 2);
    assert_eq!(page1_body["returned"], 5);

    let page3 = server.get("/fleet/api/v1/trucks?limit=2&offset=4")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    let page3_body = page3.json::<serde_json::Value>();
    assert_eq!(page3_body["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_list_trailers_pagination_windows() {
    let (server, _state, _b, _d, _rx) = setup().await;
    let owner_token = setup_owner(&server).await;

    for i in 0..5 {
        server.post("/fleet/api/v1/trailers")
            .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
            .json(&serde_json::json!({ "unit_number": format!("TRL-PG-{i}"), "owner": "fleet" }))
            .await;
    }

    let page1 = server.get("/fleet/api/v1/trailers?limit=2&offset=0")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    let page1_body = page1.json::<serde_json::Value>();
    assert_eq!(page1_body["items"].as_array().unwrap().len(), 2);
    assert_eq!(page1_body["returned"], 5);

    let page3 = server.get("/fleet/api/v1/trailers?limit=2&offset=4")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    let page3_body = page3.json::<serde_json::Value>();
    assert_eq!(page3_body["items"].as_array().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// Loads — locks the new explicit `total` field on DispatchLoadListResponse.
// ---------------------------------------------------------------------------

async fn create_test_facility(server: &TestServer, owner_token: &str, name: &str, address: &str) -> String {
    server.post("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": name, "address": address }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_list_loads_total_field_matches_full_count() {
    let (server, _state, _b, _d, _rx) = setup().await;
    let owner_token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, &owner_token, "Pager Dock", "Memphis, TN").await;

    for i in 0..3 {
        let resp = server.post("/fleet/api/v1/loads")
            .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
            .json(&serde_json::json!({
                "customer_name": format!("Pager Customer {i}"),
                "stops": [{
                    "sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                    "facility_id": fac_id, "scheduled_arrive": "2026-06-01T08:00:00",
                    "timezone": "America/Chicago"
                }],
                "rate_items": []
            }))
            .await;
        assert_eq!(resp.status_code(), 201);
    }

    let page = server.get("/fleet/api/v1/loads?limit=2&offset=0")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(page.status_code(), 200);
    let body = page.json::<serde_json::Value>();
    assert_eq!(body["items"].as_array().unwrap().len(), 2, "page respects limit");
    assert_eq!(body["total"], 3, "new `total` field carries the full matching count");
    assert_eq!(body["returned"], 3, "`returned` kept as-is for backward compat (already carried total)");
}

// ---------------------------------------------------------------------------
// Trips — locks limit/offset threading and the new `returned`/`total` fields
// on the trips list (previously an unbounded full scan with per-request
// enrichment of every trip).
// ---------------------------------------------------------------------------

async fn create_test_trip(server: &TestServer, owner_token: &str) -> serde_json::Value {
    let resp = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({}))
        .await;
    assert_eq!(resp.status_code(), 201, "trip creation failed");
    resp.json::<serde_json::Value>()
}

#[tokio::test]
async fn test_list_trips_pagination_windows_and_total() {
    let (server, _state, _b, _d, _rx) = setup().await;
    let owner_token = setup_owner(&server).await;

    for _ in 0..5 {
        create_test_trip(&server, &owner_token).await;
    }

    let page1 = server.get("/fleet/api/v1/trips?limit=2&offset=0")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(page1.status_code(), 200);
    let page1_body = page1.json::<serde_json::Value>();
    assert_eq!(page1_body["items"].as_array().unwrap().len(), 2, "page1 must return exactly `limit` items");
    assert_eq!(page1_body["total"], 5, "`total` carries the full matching count");
    assert_eq!(page1_body["returned"], 2, "`returned` is the count on this page");

    let page2 = server.get("/fleet/api/v1/trips?limit=2&offset=2")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    let page2_body = page2.json::<serde_json::Value>();
    assert_eq!(page2_body["items"].as_array().unwrap().len(), 2);

    let page3 = server.get("/fleet/api/v1/trips?limit=2&offset=4")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    let page3_body = page3.json::<serde_json::Value>();
    assert_eq!(page3_body["items"].as_array().unwrap().len(), 1, "last page holds the remainder (5 - 4)");
    assert_eq!(page3_body["total"], 5);

    let mut ids: HashSet<String> = HashSet::new();
    for body in [&page1_body, &page2_body, &page3_body] {
        for item in body["items"].as_array().unwrap() {
            let id = item["id"].as_str().unwrap().to_string();
            assert!(ids.insert(id), "trip appeared in more than one page — windows overlap");
        }
    }
    assert_eq!(ids.len(), 5);
}

#[tokio::test]
async fn test_list_trips_default_limit_and_trip_number_filter() {
    let (server, _state, _b, _d, _rx) = setup().await;
    let owner_token = setup_owner(&server).await;

    let mut trip_numbers = Vec::new();
    for _ in 0..3 {
        let trip = create_test_trip(&server, &owner_token).await;
        trip_numbers.push(trip["trip_number"].as_str().unwrap().to_string());
    }

    let resp = server.get("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["items"].as_array().unwrap().len(), 3);
    assert_eq!(body["total"], 3);
    assert_eq!(body["returned"], 3);

    // trip_number now filters in the DB query, so `total` reflects the filter.
    let target = &trip_numbers[1];
    let filtered = server.get(&format!("/fleet/api/v1/trips?trip_number={target}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    let filtered_body = filtered.json::<serde_json::Value>();
    assert_eq!(filtered_body["items"].as_array().unwrap().len(), 1);
    assert_eq!(filtered_body["total"], 1);
    assert_eq!(filtered_body["items"][0]["trip_number"].as_str().unwrap(), target);
}
