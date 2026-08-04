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
/// summary is still readable, listable and attachable, so no API path needs
/// recovery to have finished.
pub async fn serve(state: AppState, listener: tokio::net::TcpListener) -> std::io::Result<()> {
    let startup = state.clone();
    tokio::spawn(async move {
        // Indices first: they are bounded work, and building them before recovery
        // starts writing keeps the window where they contend with pipeline writes
        // as small as possible.
        create_search_indices(&startup.db).await;
        // Recover facilities persisted without an embedding (e.g. embed model down
        // at create, or geocode-skipped) so they become searchable for dedup again.
        spawn_facility_embedding_backfill(startup.db.clone(), startup.ai.clone());
        if let Err(e) = requeue_stale(&startup.db, &startup.pipeline_tx, &startup.geocoding_tx, &startup.routing_tx).await {
            // Non-fatal on purpose. An unrecovered backlog means stale summaries,
            // which is not a reason to take the whole API down — that trade is the
            // entire point of #404.
            tracing::error!("startup recovery failed: {e}");
        }
    });

    axum::serve(listener, crate::api::router(state)).await
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
