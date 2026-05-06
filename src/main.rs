// src/main.rs
use ollie::{
    ai::OllamaClient,
    api,
    config::Config,
    db::DbClient,
    pipeline::{recovery::requeue_stale, spawn_pipeline},
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
        &config.ollama_base_url,
        &config.ollama_embed_model,
        &config.ollama_summary_model,
        &config.ollama_vision_model,
    ));

    let pipeline_tx = spawn_pipeline(config.pipeline_workers, db.clone(), store.clone(), ai.clone());
    requeue_stale(&db, &pipeline_tx).await?;

    // Build vector index (no-op if table is empty or index already exists)
    if let Err(e) = db.create_vector_index().await {
        tracing::warn!("vector index not created: {e} (needs data first)");
    }

    let state = AppState { db, store, ai, pipeline_tx, config: config.clone() };
    let app = api::router(state);

    let addr: SocketAddr = format!("0.0.0.0:{}", config.port).parse()?;
    tracing::info!("listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
