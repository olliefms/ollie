// src/main.rs
use ollie::{
    ai::OllamaClient,
    api,
    config::Config,
    db::DbClient,
    geocoding::GeocodingClient,
    pipeline::{recovery::requeue_stale, spawn_pipeline, spawn_geocoding_pipeline, spawn_routing_pipeline},
    routing::RoutingClient,
    storage::BlobStore,
    AppState,
};
use std::{net::SocketAddr, sync::Arc};

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
        (db.create_event_vector_index().await, "events"),
    ] {
        if let Err(e) = result {
            tracing::warn!("vector index not created for {label}: {e}");
        }
    }
    if let Err(e) = db.create_event_scalar_indices().await {
        tracing::warn!("scalar indices not created for events: {e}");
    }

    let state = AppState {
        db, store, ai, geocoding, ors,
        pipeline_tx, geocoding_tx, routing_tx,
        config: config.clone(),
    };
    let app = api::router(state);

    let addr: SocketAddr = format!("0.0.0.0:{}", config.port).parse()?;
    tracing::info!("listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
