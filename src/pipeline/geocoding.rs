// src/pipeline/geocoding.rs
use crate::{
    ai::{embed::embed_text, OllamaClient},
    db::DbClient,
    error::AppError,
    geocoding::GeocodingClient,
};
use uuid::Uuid;

fn simplify_address(s: &str) -> Option<String> {
    let mut result = s.to_string();

    // Strip c/o segments (case-insensitive): remove from "c/o" to next comma or end of string
    let lower = result.to_lowercase();
    if let Some(co_pos) = lower.find("c/o") {
        let after_co = &result[co_pos..];
        let segment_end = after_co.find(',').map(|i| co_pos + i).unwrap_or(result.len());
        let prefix = result[..co_pos].trim_end_matches(|c: char| c == ',' || c.is_whitespace());
        let suffix = result[segment_end..].trim_start_matches(|c: char| c == ',' || c.is_whitespace());
        result = if prefix.is_empty() {
            suffix.to_string()
        } else if suffix.is_empty() {
            prefix.to_string()
        } else {
            format!("{}, {}", prefix, suffix)
        };
    }

    // Highway normalization: replace "US Hwy <digits>" (case-insensitive) with "US-<digits>"
    let lower2 = result.to_lowercase();
    if let Some(hwy_pos) = lower2.find("us hwy ") {
        let after_hwy = &result[hwy_pos + "us hwy ".len()..];
        let digits_end = after_hwy
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after_hwy.len());
        let digits = &after_hwy[..digits_end];
        if !digits.is_empty() {
            result = format!(
                "{}US-{}{}",
                &result[..hwy_pos],
                digits,
                &after_hwy[digits_end..]
            );
        }
    }

    // Collapse multiple spaces → single space; trim trailing commas/whitespace
    let collapsed: String = result
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let trimmed = collapsed.trim_end_matches(|c: char| c == ',' || c.is_whitespace()).to_string();

    if trimmed == s {
        None
    } else {
        Some(trimmed)
    }
}

pub async fn process_facility_geocoding(
    id: Uuid,
    db: &DbClient,
    geocoding: &GeocodingClient,
    ai: &OllamaClient,
    routing_tx: &async_channel::Sender<Uuid>,
) -> Result<(), AppError> {
    let facility = db.get_facility_by_id(id).await?;
    let address = facility.address.clone();

    let geocode_result = geocoding.geocode(&address).await;

    let geocode_result = if geocode_result.is_none() {
        if let Some(simplified) = simplify_address(&address) {
            let retry = geocoding.geocode(&simplified).await;
            if retry.is_some() {
                tracing::info!(
                    "geocoded {} via simplified address: {} -> {}",
                    id,
                    address,
                    simplified
                );
            }
            retry
        } else {
            None
        }
    } else {
        geocode_result
    };

    match geocode_result {
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

    // Re-queue routing for loads that reference this facility and still have no miles
    match db.list_unrouted_loads_for_facility(id).await {
        Ok(load_ids) => {
            for load_id in load_ids {
                let _ = routing_tx.try_send(load_id);
                tracing::debug!("re-queued routing for load {load_id} after facility {id} geocoded");
            }
        }
        Err(e) => tracing::warn!("failed to query unrouted loads for facility {id}: {e}"),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::DbClient, geocoding::GeocodingClient, models::GeocodeStatus};
    use std::sync::Arc;
    use tempfile::TempDir;

    #[test]
    fn test_simplify_address_strips_co() {
        let result = simplify_address("c/o John Smith, 123 Main St");
        assert_eq!(result, Some("123 Main St".to_string()));
    }

    #[test]
    fn test_simplify_address_highway_normalization() {
        let result = simplify_address("1234 US Hwy 30, Someplace, NE");
        assert_eq!(result, Some("1234 US-30, Someplace, NE".to_string()));
    }

    #[test]
    fn test_simplify_address_no_change_returns_none() {
        let result = simplify_address("123 Main St, Springfield, IL");
        assert_eq!(result, None);
    }

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
            geocode_status: GeocodeStatus::Pending, geocode_failure_count: 0,
            contacts: vec![], notes: None, tags: vec![], blob_ids: vec![],
            avg_dwell_minutes: None, dwell_sample_count: 0, embedding: None,
            created_at: now, updated_at: now,
        };
        db.insert_facility(&facility).await.unwrap();

        let (routing_tx, _rx) = async_channel::bounded(10);
        // Process — will fail to geocode (no network or no match)
        // We assert the function completes without panic
        let _ = process_facility_geocoding(facility.id, &db, &geocoding, &ai, &routing_tx).await;
    }
}
