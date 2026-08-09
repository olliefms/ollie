// tests/it/fleet_static_cache_test.rs
//
// The fleet SPA is served by a bare `ServeDir`, which sets `Last-Modified` but
// no `Cache-Control` — leaving browsers on heuristic freshness. The `?v=` stamps
// in `index.html` only cover `base.css`, `components.css`, and `app.js`, and the
// query string does not survive `app.js`'s relative `import` specifiers, so every
// module under `pages/`, `components/`, and `utils/` is fetched unversioned. A
// deploy could therefore pair a fresh `router.js` with a cached `utils/dom.js`.
//
// These tests pin the `no-cache` revalidation header that closes that gap, and
// pin that it stays scoped to the SPA assets — the JSON API under /fleet/api must
// not inherit it, and neither must the driver PWA, which does its own SW caching.

use axum::http::header;
use axum_test::TestServer;
use ollie::{ai::OllamaClient, api, config::Config, db::DbClient, storage::BlobStore, AppState};
use std::sync::Arc;
use tempfile::TempDir;
use webauthn_rs::prelude::{Url, WebauthnBuilder};

async fn setup() -> (TestServer, TempDir, TempDir, async_channel::Receiver<ollie::pipeline::PipelineJob>) {
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
    let server = TestServer::new(api::router(state)).unwrap();
    (server, blob_dir, db_dir, rx)
}

fn cache_control(resp: &axum_test::TestResponse) -> Option<String> {
    resp.headers()
        .get(header::CACHE_CONTROL)
        .map(|v| v.to_str().unwrap().to_string())
}

/// The unstamped modules are the whole point — `app.js` is stamped in
/// `index.html`, but nothing it imports is.
#[tokio::test]
async fn test_unstamped_fleet_modules_are_served_no_cache() {
    let (server, _b, _d, _rx) = setup().await;

    for path in [
        "/fleet/router.js",
        "/fleet/utils/dom.js",
        "/fleet/utils/api.js",
        "/fleet/pages/loads.js",
        "/fleet/components/confirm.js",
    ] {
        let resp = server.get(path).await;
        assert_eq!(resp.status_code(), 200, "{path} should be served");
        assert_eq!(
            cache_control(&resp).as_deref(),
            Some("no-cache"),
            "{path} must revalidate — it carries no ?v= stamp",
        );
    }
}

/// index.html itself must revalidate, or a `?v=` bump inside it is never seen.
#[tokio::test]
async fn test_fleet_index_and_stamped_assets_are_no_cache() {
    let (server, _b, _d, _rx) = setup().await;

    for path in ["/fleet/", "/fleet/index.html", "/fleet/app.js", "/fleet/css/components.css"] {
        let resp = server.get(path).await;
        assert_eq!(resp.status_code(), 200, "{path} should be served");
        assert_eq!(cache_control(&resp).as_deref(), Some("no-cache"), "{path}");
    }
}

/// The SPA fallback route must also revalidate; a cached deep-link response
/// would pin an old index.html for every client-side route.
#[tokio::test]
async fn test_fleet_spa_fallback_is_no_cache() {
    let (server, _b, _d, _rx) = setup().await;

    let resp = server.get("/fleet/loads/some-client-side-route").await;
    assert_eq!(resp.status_code(), 200, "SPA fallback should serve index.html");
    assert_eq!(cache_control(&resp).as_deref(), Some("no-cache"));
}

/// Nesting a Router at /fleet must not shadow the JSON API merged before it.
/// This is the regression that a plain `nest_service` swap could introduce.
#[tokio::test]
async fn test_fleet_api_routes_still_reachable_and_not_rewritten() {
    let (server, _b, _d, _rx) = setup().await;

    // Unauthenticated, but it must reach the API handler (401), not fall through
    // to the SPA fallback (which would return 200 + text/html).
    let resp = server.get("/fleet/api/v1/drivers").await;
    assert_eq!(
        resp.status_code(), 401,
        "API route must reach the handler, not the static fallback",
    );
    assert_eq!(
        cache_control(&resp), None,
        "no-cache must stay scoped to the SPA assets, not the JSON API",
    );

    // setup/status is unauthenticated and returns JSON — proves the merge order
    // survives and the response isn't the SPA's index.html.
    let resp = server.get("/fleet/api/v1/setup/status").await;
    assert_eq!(resp.status_code(), 200);
    assert!(
        resp.headers().get(header::CONTENT_TYPE).unwrap().to_str().unwrap().contains("json"),
        "/fleet/setup must return JSON, not the SPA fallback",
    );
}

/// `no-cache` is only cheap if revalidation actually short-circuits. Prove the
/// If-Modified-Since round trip returns a bodyless 304 rather than the file
/// again — otherwise this trades staleness for re-downloading the SPA per nav.
#[tokio::test]
async fn test_unchanged_fleet_asset_revalidates_to_304() {
    let (server, _b, _d, _rx) = setup().await;

    let first = server.get("/fleet/utils/dom.js").await;
    assert_eq!(first.status_code(), 200);
    let last_modified = first
        .headers()
        .get(header::LAST_MODIFIED)
        .expect("ServeDir must send Last-Modified for revalidation to work")
        .clone();
    assert!(!first.as_bytes().is_empty(), "first fetch should carry the file");

    let second = server
        .get("/fleet/utils/dom.js")
        .add_header(header::IF_MODIFIED_SINCE, last_modified)
        .await;
    assert_eq!(
        second.status_code(), 304,
        "unchanged asset must revalidate to 304, not re-send the body",
    );
    assert!(second.as_bytes().is_empty(), "304 must have no body");
}

/// The driver PWA keeps its existing headers — its service worker owns caching,
/// and this change deliberately does not touch it.
#[tokio::test]
async fn test_driver_static_is_unchanged() {
    let (server, _b, _d, _rx) = setup().await;

    let resp = server.get("/driver/sw.js").await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(
        cache_control(&resp), None,
        "driver assets should keep ServeDir's default headers",
    );
}
