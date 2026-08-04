// src/startup.rs
//! Startup ordering, kept out of `main.rs` so it is reachable from a test — #404
//! was a bug in the *order* these steps run, and a binary's `main` cannot be
//! called from one.

use crate::{
    db::DbClient,
    pipeline::{embedding_backfill::spawn_facility_embedding_backfill, recovery::requeue_stale},
    AppState,
};

/// Serve `state` on an already-bound listener, running every startup task whose
/// cost scales with data volume BEHIND the accept loop.
///
/// The pipeline channel is bounded, so once the backlog exceeds its capacity
/// `requeue_stale` inherits the pipeline's *drain* time — 768 stale blobs at
/// ~40 s each projected to an 8.5-hour cold start while this ran ahead of the
/// listener, with the port refusing connections the whole time (#404). Summaries,
/// embeddings and geocodes are all eventually-consistent: a blob with a pending
/// summary is still readable, listable and attachable, so no *read* path needs
/// recovery to have finished. The upload paths do touch the same saturated
/// channel — `pipeline::enqueue` is what keeps them from inheriting the drain.
pub async fn serve(state: AppState, listener: tokio::net::TcpListener) -> std::io::Result<()> {
    let task = tokio::spawn(background_startup(state.clone()));
    tokio::spawn(async move {
        // A panic in here would otherwise reach nothing but the default panic
        // hook — the process keeps serving with the backlog silently unrecovered.
        if let Err(e) = task.await {
            tracing::error!("startup background task ended abnormally: {e}");
        }
    });

    axum::serve(listener, crate::api::router(state)).await
}

async fn background_startup(state: AppState) {
    // Indices first. They are bounded work measured in seconds, while the drain
    // below is measured in hours on the cold start this exists for — delaying it
    // by an index build is noise, and building before recovery starts writing
    // keeps the window where the two contend as small as possible.
    create_search_indices(&state.db).await;
    // Recover facilities persisted without an embedding (e.g. embed model down at
    // create, or geocode-skipped) so they become searchable for dedup again.
    spawn_facility_embedding_backfill(state.db.clone(), state.ai.clone());
    if let Err(e) = requeue_stale(&state.db, &state.pipeline_tx, &state.geocoding_tx, &state.routing_tx).await {
        // Non-fatal on purpose. An unrecovered backlog means stale summaries,
        // which is not a reason to take the whole API down — that trade is the
        // entire point of #404.
        tracing::error!("startup recovery failed: {e}");
    }
}

/// Probe a local listener on `port`, the body of the `ollie healthcheck`
/// subcommand wired to the image's `HEALTHCHECK`. `Err` until the listener is
/// actually answering, so "container up" stops meaning "service reachable"
/// (#404: the container reported healthy through the entire cold start).
///
/// Deliberately shallow: it proves the API can answer, not that recovery has
/// finished — a pending backlog does not make the service unready.
pub async fn healthcheck(port: u16) -> anyhow::Result<()> {
    let url = format!("http://127.0.0.1:{port}/version");
    // `no_proxy()`: reqwest honours HTTP_PROXY/ALL_PROXY by default and does not
    // exempt loopback, so in an egress-controlled deployment the probe would be
    // routed off-box and fail a perfectly healthy container.
    let resp = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(5))
        .build()?
        .get(&url)
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("{url} returned {}", resp.status());
    }
    Ok(())
}

/// `PORT` as the healthcheck subcommand resolves it. Reads the env var directly
/// rather than `Config::from_env`, which would fail the probe for reasons that
/// have nothing to do with reachability. Must stay in step with `Config`.
pub fn healthcheck_port() -> u16 {
    std::env::var("PORT").ok().and_then(|v| v.parse::<u16>().ok()).unwrap_or(3000)
}

/// Best-effort index creation — a missing index degrades vector search to an exact
/// brute-force scan, it does not break anything, so every failure is a warning.
async fn create_search_indices(db: &DbClient) {
    for (result, label) in [
        (db.create_vector_index().await, "blobs"),
        (db.create_facility_vector_index().await, "facilities"),
        (db.create_load_vector_index().await, "loads"),
        (db.create_driver_vector_index().await, "drivers"),
        (db.create_truck_vector_index().await, "trucks"),
        (db.create_trailer_vector_index().await, "trailers"),
        (db.create_maintenance_vector_index().await, "maintenance"),
        (db.create_event_vector_index().await, "events"),
    ] {
        if let Err(e) = result {
            tracing::warn!("vector index not created for {label}: {e}");
        }
    }
    if let Err(e) = db.create_event_scalar_indices().await {
        tracing::warn!("scalar indices not created for events: {e}");
    }
}
