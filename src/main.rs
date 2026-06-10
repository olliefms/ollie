// src/main.rs
use ollie::{
    ai::OllamaClient,
    api,
    config::Config,
    db::DbClient,
    geocoding::GeocodingClient,
    pipeline::{embedding_backfill::spawn_facility_embedding_backfill, recovery::requeue_stale, spawn_pipeline, spawn_geocoding_pipeline, spawn_routing_pipeline},
    routing::RoutingClient,
    storage::BlobStore,
    AppState,
};
use std::{net::SocketAddr, sync::Arc};
use webauthn_rs::prelude::{Url, WebauthnBuilder};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ollie=info".into()),
        )
        .init();

    let config = Arc::new(Config::from_env().map_err(|e| anyhow::anyhow!(e))?);

    let db = Arc::new(DbClient::new(&config.lancedb_path, config.ollama_embed_dim).await?);
    let store = Arc::new(BlobStore::new(&config.blob_store_path));
    let ai = Arc::new(OllamaClient::new(
        &config.ollama_base_url, &config.ollama_embed_model,
        &config.ollama_summary_model, &config.ollama_vision_model,
    ));
    let geocoding = Arc::new(GeocodingClient::new());
    let ors = Arc::new(RoutingClient::new(&config.ors_api_key));

    let pipeline_tx = spawn_pipeline(config.pipeline_workers, db.clone(), store.clone(), ai.clone(), config.extract_store_path.clone());
    let routing_tx = spawn_routing_pipeline(1, db.clone(), ors.clone());
    let geocoding_tx = spawn_geocoding_pipeline(config.geocoding_workers, db.clone(), geocoding.clone(), ai.clone(), routing_tx.clone());

    requeue_stale(&db, &pipeline_tx, &geocoding_tx, &routing_tx).await?;

    for (result, label) in [
        (db.create_vector_index().await, "blobs"),
        (db.create_facility_vector_index().await, "facilities"),
        (db.create_load_vector_index().await, "loads"),
        (db.create_driver_vector_index().await, "drivers"),
        (db.create_truck_vector_index().await, "trucks"),
        (db.create_trailer_vector_index().await, "trailers"),
        (db.create_event_vector_index().await, "events"),
    ] {
        if let Err(e) = result {
            tracing::warn!("vector index not created for {label}: {e}");
        }
    }
    if let Err(e) = db.create_event_scalar_indices().await {
        tracing::warn!("scalar indices not created for events: {e}");
    }

    // Recover facilities persisted without an embedding (e.g. embed model down
    // at create, or geocode-skipped) so they become searchable for dedup again.
    spawn_facility_embedding_backfill(db.clone(), ai.clone());

    let rp_origin = Url::parse(&config.driver_rp_origin)
        .expect("DRIVER_RP_ORIGIN must be a valid URL");
    let webauthn = Arc::new(
        WebauthnBuilder::new(&config.driver_rp_id, &rp_origin)
            .expect("Failed to build Webauthn")
            .build()
            .expect("Failed to build Webauthn"),
    );

    let auth_challenge_store: Arc<dashmap::DashMap<uuid::Uuid, _>> = Arc::new(dashmap::DashMap::new());
    let reg_challenge_store: Arc<dashmap::DashMap<uuid::Uuid, _>> = Arc::new(dashmap::DashMap::new());

    let auth_store_sweep = auth_challenge_store.clone();
    let reg_store_sweep = reg_challenge_store.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            let cutoff = std::time::Instant::now() - std::time::Duration::from_secs(300);
            auth_store_sweep.retain(|_, (_, ts)| *ts > cutoff);
            reg_store_sweep.retain(|_, (_, ts)| *ts > cutoff);
        }
    });

    let state = AppState {
        db, store, ai, geocoding, ors,
        pipeline_tx, geocoding_tx, routing_tx,
        config: config.clone(),
        webauthn,
        auth_challenge_store,
        reg_challenge_store,
    };
    let app = api::router(state);

    let addr: SocketAddr = format!("0.0.0.0:{}", config.port).parse()?;
    tracing::info!("ollie v{}", env!("CARGO_PKG_VERSION"));
    tracing::info!("listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
