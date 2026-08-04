// tests/startup_recovery_test.rs
//! #404: startup pipeline recovery must run *behind* the bound HTTP listener.
//!
//! The pipeline channel is bounded, so requeueing a backlog larger than its
//! capacity blocks until the workers drain it. While that ran ahead of the bind,
//! a restart with a summarisation backlog refused every connection for as long as
//! the drain took (a projected 8.5 hours for 768 blobs) with no listener at all —
//! and Docker reported the container healthy throughout.
//!
//! This test reproduces the shape of that outage: a full pipeline channel that
//! nothing is draining, and a `requeue_stale` that therefore cannot finish. The
//! API must answer anyway.

use chrono::Utc;
use ollie::{
    ai::OllamaClient,
    api,
    config::Config,
    db::DbClient,
    models::{BlobRecord, BlobStatus},
    pipeline::recovery::requeue_stale,
    storage::BlobStore,
    AppState,
};
use std::sync::Arc;
use tempfile::TempDir;
use uuid::Uuid;
use webauthn_rs::prelude::{Url, WebauthnBuilder};

/// Small enough that the backlog below is guaranteed to wedge the send loop.
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
        "http://127.0.0.1:1", "nomic-embed-text", "llama3.2", "moondream",
    ));
    let geocoding = Arc::new(ollie::geocoding::GeocodingClient::new());
    let ors = Arc::new(ollie::routing::RoutingClient::new(""));

    for n in 0..BACKLOG {
        db.insert(&pending_blob(n)).await.unwrap();
    }

    // No receiver is ever polled — this stands in for pipeline workers that are
    // far slower than the requeue loop.
    let (pipeline_tx, _rx) = async_channel::bounded(CHANNEL_CAPACITY);
    let (geocoding_tx, _grx) = async_channel::bounded(CHANNEL_CAPACITY);
    let (routing_tx, _rrx) = async_channel::bounded(CHANNEL_CAPACITY);

    let rp_origin = Url::parse("http://localhost:3000").unwrap();
    let webauthn = Arc::new(WebauthnBuilder::new("localhost", &rp_origin).unwrap().build().unwrap());

    let state = AppState {
        db: db.clone(), store, ai, geocoding, ors,
        pipeline_tx: pipeline_tx.clone(),
        geocoding_tx: geocoding_tx.clone(),
        routing_tx: routing_tx.clone(),
        config,
        webauthn,
        auth_challenge_store: Arc::new(dashmap::DashMap::new()),
        reg_challenge_store: Arc::new(dashmap::DashMap::new()),
    };

    // Mirror main.rs: bind and serve first, then kick recovery off behind it.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, api::router(state)).await.unwrap();
    });

    let recovery = tokio::spawn(async move {
        requeue_stale(&db, &pipeline_tx, &geocoding_tx, &routing_tx).await
    });

    let resp = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        reqwest::get(format!("http://{addr}/version")),
    )
    .await
    .expect("GET /version timed out — the listener is blocked behind startup recovery")
    .expect("GET /version failed — nothing is listening");
    assert_eq!(resp.status().as_u16(), 200);

    assert!(
        !recovery.is_finished(),
        "recovery finished, so the backlog never wedged the send loop — the test no longer reproduces #404",
    );
    recovery.abort();
}
