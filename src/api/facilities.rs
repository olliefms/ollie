// src/api/facilities.rs
use crate::{
    ai::embed::embed_text,
    error::AppError,
    models::{FacilityRecord, FacilityResolutionResponse, GeocodeStatus},
    AppState,
};
use chrono::Utc;
use uuid::Uuid;

/// Resolve a facility from a name+address string, applying dedup logic.
/// Returns Ok(Uuid) if resolved/created.
/// Returns Err(AppError::FacilityResolution) if ambiguous (stop_index defaults to 0; caller overrides).
/// Returns Err on embed or DB failure — fails closed rather than silently creating duplicates.
pub async fn resolve_or_create_facility(
    state: &AppState,
    name: &str,
    address: &str,
    force_new: bool,
) -> Result<Uuid, AppError> {
    if force_new {
        return create_new_facility(state, name, address).await;
    }

    let text = format!("{name} {address}");
    let embedding = embed_text(&state.ai, &text).await?;

    let candidates = state.db.search_facilities(embedding, None, &[], 5).await?;

    let high = state.config.facility_dedup_high_threshold as f32;
    let low = state.config.facility_dedup_low_threshold as f32;

    if let Some(top) = candidates.first() {
        if top.score.unwrap_or(0.0) >= high {
            return Ok(top.id);
        }
    }

    let above_low: Vec<_> = candidates.into_iter()
        .filter(|c| c.score.unwrap_or(0.0) >= low)
        .map(|c| crate::models::FacilityCandidate {
            id: c.id, name: c.name, address: c.address,
            normalized_address: c.normalized_address,
            score: c.score.unwrap_or(0.0),
        })
        .collect();

    if !above_low.is_empty() {
        return Err(AppError::FacilityResolution(Box::new(vec![FacilityResolutionResponse {
            stop_index: 0,
            facility_resolution_required: true,
            candidates: above_low,
        }])));
    }

    create_new_facility(state, name, address).await
}

async fn create_new_facility(
    state: &AppState,
    name: &str,
    address: &str,
) -> Result<Uuid, AppError> {
    let now = Utc::now();
    let text = format!("{name} {address}");
    let embedding = embed_text(&state.ai, &text).await.ok();
    let record = FacilityRecord {
        id: Uuid::new_v4(), owner_id: 0,
        name: name.to_string(), address: address.to_string(),
        normalized_address: None, lat: None, lng: None,
        geocode_status: GeocodeStatus::Pending, geocode_failure_count: 0,
        contacts: vec![], notes: None, tags: vec![], blob_ids: vec![],
        avg_dwell_minutes: None, dwell_sample_count: 0, archived: false,
        embedding, created_at: now, updated_at: now,
    };
    state.db.insert_facility(&record).await?;
    let _ = state.geocoding_tx.try_send(record.id);
    Ok(record.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ai::OllamaClient, config::Config, db::DbClient, routing::RoutingClient,
        storage::BlobStore,
    };
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn test_state() -> (AppState, TempDir, TempDir) {
        let blob_dir = TempDir::new().unwrap();
        let db_dir = TempDir::new().unwrap();
        std::env::set_var("DRIVER_JWT_SECRET", "test-driver-jwt-secret-that-is-long-enough");
        std::env::set_var("FLEET_JWT_SECRET", "test-fleet_user-jwt-secret-that-is-long-enough");
        std::env::set_var("DRIVER_RP_ID", "localhost");
        std::env::set_var("DRIVER_RP_ORIGIN", "http://localhost:3000");
        let config = Arc::new(Config::from_env().unwrap());
        let db = Arc::new(DbClient::new(db_dir.path().to_str().unwrap(), 4).await.unwrap());
        let store = Arc::new(BlobStore::new(blob_dir.path().to_str().unwrap()));
        let ai = Arc::new(OllamaClient::new(
            "http://localhost:11434", "nomic-embed-text", "llama3.2", "llava",
        ));
        let geocoding = Arc::new(crate::geocoding::GeocodingClient::new());
        let ors = Arc::new(RoutingClient::new(""));
        let (geocoding_tx, _rx) = async_channel::bounded(10);
        let (routing_tx, _rx2) = async_channel::bounded(10);
        let (pipeline_tx, _rx3) = async_channel::bounded(10);
        let rp_origin = webauthn_rs::prelude::Url::parse("http://localhost:3000").unwrap();
        let webauthn = Arc::new(
            webauthn_rs::prelude::WebauthnBuilder::new("localhost", &rp_origin)
                .unwrap()
                .build()
                .unwrap(),
        );
        let auth_challenge_store = Arc::new(dashmap::DashMap::new());
        let reg_challenge_store = Arc::new(dashmap::DashMap::new());
        let state = AppState {
            db, store, ai, geocoding, ors,
            pipeline_tx, geocoding_tx, routing_tx,
            config, webauthn,
            auth_challenge_store,
            reg_challenge_store,
        };
        (state, blob_dir, db_dir)
    }

    #[tokio::test]
    async fn test_resolve_force_new_creates_facility() {
        let (state, _b, _d) = test_state().await;
        let id = resolve_or_create_facility(&state, "Fresh Dock", "123 Main St, Dallas TX", true)
            .await
            .expect("force_new should create a new facility even without Ollama");
        state.db.get_facility_by_id(id).await.expect("facility should exist");
    }

    #[tokio::test]
    async fn test_resolve_propagates_error_when_embed_fails() {
        let (state, _b, _d) = test_state().await;
        // Without Ollama, embed_text fails and the error is propagated (fail closed)
        let result = resolve_or_create_facility(&state, "Dock A", "100 Oak Ave, Nashville TN", false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_resolve_force_new_skips_dedup() {
        let (state, _b, _d) = test_state().await;
        let id1 = resolve_or_create_facility(&state, "Dock B", "200 Elm St, Atlanta GA", true).await.unwrap();
        let id2 = resolve_or_create_facility(&state, "Dock B", "200 Elm St, Atlanta GA", true).await.unwrap();
        assert_ne!(id1, id2, "force_new_facility=true must always create a new record");
    }
}
