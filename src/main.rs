// src/main.rs
use ollie::{
    ai::OllamaClient,
    config::Config,
    db::DbClient,
    geocoding::GeocodingClient,
    pipeline::{spawn_pipeline, spawn_geocoding_pipeline, spawn_routing_pipeline},
    routing::RoutingClient,
    storage::BlobStore,
    startup,
    AppState,
};
use std::{net::SocketAddr, sync::Arc};
use webauthn_rs::prelude::{Url, WebauthnBuilder};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return healthcheck().await;
    }
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

    // Bind before anything that scales with data volume — `startup::serve` owns
    // that ordering and is where the reasoning lives (#404).
    let addr: SocketAddr = format!("0.0.0.0:{}", config.port).parse()?;
    tracing::info!("ollie v{}", env!("CARGO_PKG_VERSION"));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("listening on {addr}");
    startup::serve(state, listener).await?;
    Ok(())
}

/// `ollie healthcheck` — the image's HEALTHCHECK command. Exits non-zero until the
/// HTTP listener is actually answering, so "container up" stops meaning "service
/// reachable" (#404: the container reported healthy through the entire cold start).
/// Reads `PORT` directly rather than `Config::from_env`, which would fail the check
/// for reasons that have nothing to do with reachability.
async fn healthcheck() -> anyhow::Result<()> {
    let port = std::env::var("PORT").ok().and_then(|v| v.parse::<u16>().ok()).unwrap_or(3000);
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
