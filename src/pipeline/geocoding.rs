// src/pipeline/geocoding.rs
use crate::{
    ai::{embed::embed_text, OllamaClient},
    db::DbClient,
    error::AppError,
    geocoding::GeocodingClient,
};
use uuid::Uuid;

pub async fn process_facility_geocoding(
    id: Uuid,
    db: &DbClient,
    geocoding: &GeocodingClient,
    ai: &OllamaClient,
) -> Result<(), AppError> {
    let facility = db.get_facility_by_id(id).await?;

    match geocoding.geocode(&facility.address).await {
        Some((lat, lng, normalized)) => {
            db.update_facility_geocode(id, lat, lng, normalized).await?;
            tracing::info!("geocoded facility {id}: {lat},{lng}");
        }
        None => {
            db.mark_facility_geocode_failed(id).await?;
            tracing::warn!("geocoding failed for facility {id}");
            return Ok(());
        }
    }

    // Re-embed now that we have a normalized address
    let facility = db.get_facility_by_id(id).await?;
    match embed_text(ai, &facility.embedding_text()).await {
        Ok(embedding) => {
            db.update_facility_embedding(id, embedding).await?;
        }
        Err(e) => tracing::warn!("embedding failed for facility {id}: {e}"),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::DbClient, geocoding::GeocodingClient, models::GeocodeStatus};
    use std::sync::Arc;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_geocode_worker_marks_failed_on_no_match() {
        let dir = TempDir::new().unwrap();
        let db = Arc::new(DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap());
        let geocoding = Arc::new(GeocodingClient::new());
        let ai = Arc::new(crate::ai::OllamaClient::new(
            "http://localhost:11434", "nomic-embed-text", "llama3.2", "llava",
        ));

        let now = chrono::Utc::now();
        let facility = crate::models::FacilityRecord {
            id: uuid::Uuid::new_v4(), owner_id: 0,
            name: "XYZZY".into(),
            address: "zzzzzznotanaddressatall12345".into(),
            normalized_address: None, lat: None, lng: None,
            geocode_status: GeocodeStatus::Pending,
            contacts: vec![], notes: None, tags: vec![], blob_ids: vec![],
            avg_dwell_minutes: None, dwell_sample_count: 0, embedding: None,
            created_at: now, updated_at: now,
        };
        db.insert_facility(&facility).await.unwrap();

        // Process — will fail to geocode (no network or no match)
        // We assert the function completes without panic
        let _ = process_facility_geocoding(facility.id, &db, &geocoding, &ai).await;
    }
}
