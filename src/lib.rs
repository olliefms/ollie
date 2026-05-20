// src/lib.rs
pub mod ai;
pub mod api;
pub mod config;
pub mod db;
pub mod error;
pub mod events;
pub mod geocoding;
pub mod models;
pub mod pipeline;
pub mod routing;
pub mod services;
pub mod storage;

use ai::OllamaClient;
use config::Config;
use db::DbClient;
use geocoding::GeocodingClient;
use routing::RoutingClient;
use std::sync::Arc;
use std::time::Instant;
use storage::BlobStore;
use uuid::Uuid;
use webauthn_rs::prelude::{PasskeyAuthentication, PasskeyRegistration};

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<DbClient>,
    pub store: Arc<BlobStore>,
    pub ai: Arc<OllamaClient>,
    pub geocoding: Arc<GeocodingClient>,
    pub ors: Arc<RoutingClient>,
    pub pipeline_tx: async_channel::Sender<Uuid>,
    pub geocoding_tx: async_channel::Sender<Uuid>,
    pub routing_tx: async_channel::Sender<Uuid>,
    pub config: Arc<Config>,
    pub webauthn: Arc<webauthn_rs::Webauthn>,
    pub auth_challenge_store: Arc<dashmap::DashMap<Uuid, (PasskeyAuthentication, Instant)>>,
    pub reg_challenge_store: Arc<dashmap::DashMap<Uuid, (PasskeyRegistration, Instant)>>,
}
