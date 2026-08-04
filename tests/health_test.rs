// tests/health_test.rs
//
// A pipeline worker pool can die while the HTTP server keeps answering
// perfectly: a panic in a job unwinds the worker task, dropping the last
// receiver, and from then on every enqueue fails with "sending into a closed
// channel" while `/version` still returns 200. `/healthz` is what makes that
// state visible from outside — these tests pin that it reports live workers and
// flips to 503 when a pool is gone.

use axum_test::TestServer;
use ollie::{ai::OllamaClient, api, config::Config, db::DbClient, storage::BlobStore, AppState};
use std::sync::Arc;
use tempfile::TempDir;
use webauthn_rs::prelude::{Url, WebauthnBuilder};

type Receivers = (
    async_channel::Receiver<ollie::pipeline::PipelineJob>,
    async_channel::Receiver<uuid::Uuid>,
    async_channel::Receiver<uuid::Uuid>,
);

async fn setup() -> (TestServer, TempDir, TempDir, Receivers) {
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
        "http://127.0.0.1:1", "nomic-embed-text", "llama3.2", "moondream",
    ));
    let geocoding = Arc::new(ollie::geocoding::GeocodingClient::new());
    let ors = Arc::new(ollie::routing::RoutingClient::new(""));
    let (pipeline_tx, prx) = async_channel::bounded(100);
    let (geocoding_tx, grx) = async_channel::bounded(100);
    let (routing_tx, rrx) = async_channel::bounded(100);
    let rp_origin = Url::parse("http://localhost:3000").unwrap();
    let webauthn = Arc::new(WebauthnBuilder::new("localhost", &rp_origin).unwrap().build().unwrap());

    let state = AppState {
        db, store, ai, geocoding, ors,
        pipeline_tx, geocoding_tx, routing_tx, config,
        webauthn,
        auth_challenge_store: Arc::new(dashmap::DashMap::new()),
        reg_challenge_store: Arc::new(dashmap::DashMap::new()),
    };
    let server = TestServer::new(api::router(state)).unwrap();
    (server, blob_dir, db_dir, (prx, grx, rrx))
}

#[tokio::test]
async fn test_healthz_is_public_and_reports_live_workers() {
    let (server, _b, _d, _rx) = setup().await;

    let resp = server.get("/healthz").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["document_pipeline"]["workers"], 1);
    assert_eq!(body["document_pipeline"]["queued"], 0);
    assert_eq!(body["document_pipeline"]["closed"], false);
}

#[tokio::test]
async fn test_healthz_reports_503_when_the_pipeline_has_no_workers() {
    let (server, _b, _d, (prx, grx, rrx)) = setup().await;

    // Exactly what a panicking worker does: the last receiver goes away.
    drop(prx);

    let resp = server.get("/healthz").await;
    resp.assert_status(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "degraded");
    assert_eq!(body["document_pipeline"]["workers"], 0);
    assert_eq!(body["document_pipeline"]["closed"], true);
    // The other pools are independent and must still read healthy.
    assert_eq!(body["geocoding_pipeline"]["closed"], false);
    assert_eq!(body["routing_pipeline"]["closed"], false);
    drop((grx, rrx));
}
