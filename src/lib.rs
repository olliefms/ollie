// src/lib.rs
pub mod ai;
pub mod api;
pub mod config;
pub mod db;
pub mod error;
pub mod geocoding;
pub mod models;
pub mod pipeline;
pub mod storage;

use ai::OllamaClient;
use config::Config;
use db::DbClient;
use std::sync::Arc;
use storage::BlobStore;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<DbClient>,
    pub store: Arc<BlobStore>,
    pub ai: Arc<OllamaClient>,
    pub pipeline_tx: async_channel::Sender<Uuid>,
    pub config: Arc<Config>,
}
