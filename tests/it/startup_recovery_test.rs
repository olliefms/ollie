// tests/startup_recovery_test.rs
//! #404: startup pipeline recovery must run *behind* the accept loop.
//!
//! The pipeline channel is bounded, so requeueing a backlog larger than its
//! capacity blocks until the workers drain it. While that ran ahead of serving,
//! a restart with a summarisation backlog left the API unreachable for as long as
//! the drain took (a projected 8.5 hours for 768 blobs) — and Docker reported the
//! container healthy throughout.
//!
//! The test drives `startup::serve`, the function that owns the ordering, so
//! moving recovery back in front of `axum::serve` fails it.

use chrono::Utc;
use ollie::{
    ai::OllamaClient,
    config::Config,
    db::DbClient,
    models::{BlobRecord, BlobStatus},
    startup,
    storage::BlobStore,
    AppState,
};
use std::sync::Arc;
use tempfile::TempDir;
use uuid::Uuid;
use webauthn_rs::prelude::{Url, WebauthnBuilder};

/// Small enough that the backlog below is guaranteed to wedge the requeue loop.
const CHANNEL_CAPACITY: usize = 4;
const BACKLOG: usize = 16;

fn pending_blob(n: usize) -> BlobRecord {
    let now = Utc::now();
    BlobRecord {
        id: Uuid::new_v4(), owner_id: 0, checksum: format!("stale-{n}"),
        name: format!("scan-{n}.pdf"), mime_type: "application/pdf".into(), size: 1,
        status: BlobStatus::Pending, error: None, summary: None,
        tags: vec![], embedding: None, created_at: now, updated_at: now,
        visibility: Default::default(), uploaded_by: None,
    }
}

#[tokio::test]
async fn test_http_listener_serves_while_startup_recovery_is_blocked() {
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
        // Deliberately unreachable: this test must not depend on a live Ollama.
        "http://127.0.0.1:1", "nomic-embed-text", "llama3.2", "moondream",
    ));
    let geocoding = Arc::new(ollie::geocoding::GeocodingClient::new());
    let ors = Arc::new(ollie::routing::RoutingClient::new(""));

    for n in 0..BACKLOG {
        db.insert(&pending_blob(n)).await.unwrap();
    }

    // No receiver is ever polled — this stands in for pipeline workers that are
    // far slower than the requeue loop. Bound to named locals so the receivers
    // stay alive and sends block rather than error.
    let (pipeline_tx, rx) = async_channel::bounded(CHANNEL_CAPACITY);
    let (geocoding_tx, _grx) = async_channel::bounded(CHANNEL_CAPACITY);
    let (routing_tx, _rrx) = async_channel::bounded(CHANNEL_CAPACITY);

    let rp_origin = Url::parse("http://localhost:3000").unwrap();
    let webauthn = Arc::new(WebauthnBuilder::new("localhost", &rp_origin).unwrap().build().unwrap());

    let state = AppState {
        db, store, ai, geocoding, ors,
        pipeline_tx, geocoding_tx, routing_tx, config,
        webauthn,
        auth_challenge_store: Arc::new(dashmap::DashMap::new()),
        reg_challenge_store: Arc::new(dashmap::DashMap::new()),
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { startup::serve(state, listener).await.unwrap() });

    // Wait until the requeue loop is wedged on the full channel — this is the
    // state #404 spent hours in, and it must be reached for the assertion below
    // to mean anything.
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while rx.len() < CHANNEL_CAPACITY {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("startup recovery never filled the pipeline channel — the test no longer reproduces #404");

    let resp = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        reqwest::Client::builder().no_proxy().build().unwrap()
            .get(format!("http://{addr}/version")).send(),
    )
    .await
    .expect("GET /version timed out — the accept loop is stuck behind startup recovery")
    .expect("GET /version failed — nothing is serving");
    assert_eq!(resp.status().as_u16(), 200);

    // Still wedged: the API answered *during* recovery, not after it.
    assert_eq!(rx.len(), CHANNEL_CAPACITY, "the requeue loop drained unexpectedly");

    // The image's HEALTHCHECK probe must agree, against the same server — a probe
    // that reported unhealthy here would strand every deployment behind a
    // `service_healthy` condition.
    startup::healthcheck(addr.port()).await.expect("healthcheck failed against a serving listener");
}

/// The probe must be able to say "no": a dead port has to fail it, or the
/// HEALTHCHECK reports healthy exactly as #404 did.
#[tokio::test]
async fn test_healthcheck_fails_when_nothing_is_listening() {
    // Bind then drop, so the port is known-free rather than guessed.
    let port = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    };
    assert!(startup::healthcheck(port).await.is_err());
}

/// `ollie healthcheck` must resolve `PORT` the way the server does, or the probe
/// aims at the wrong port on every non-default deployment.
#[test]
fn test_healthcheck_port_matches_config_default() {
    let _env = crate::common::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var("PORT");
    assert_eq!(startup::healthcheck_port(), 3000);
    std::env::set_var("PORT", "8081");
    assert_eq!(startup::healthcheck_port(), 8081);
    std::env::remove_var("PORT");
}
