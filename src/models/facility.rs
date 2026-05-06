// src/models/facility.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GeocodeStatus {
    Pending,
    Ready,
    Failed,
}

impl GeocodeStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

impl std::str::FromStr for GeocodeStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "ready" => Ok(Self::Ready),
            "failed" => Ok(Self::Failed),
            other => Err(format!("unknown geocode status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FacilityContact {
    pub name: String,
    pub title: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FacilityRecord {
    pub id: Uuid,
    pub owner_id: i64,
    pub name: String,
    pub address: String,
    pub normalized_address: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub geocode_status: GeocodeStatus,
    pub contacts: Vec<FacilityContact>,
    pub notes: Option<String>,
    pub tags: Vec<String>,
    pub blob_ids: Vec<Uuid>,
    pub avg_dwell_minutes: Option<f64>,
    pub dwell_sample_count: i64,
    #[serde(skip)]
    pub embedding: Option<Vec<f32>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl FacilityRecord {
    pub fn embedding_text(&self) -> String {
        let contact_text = self.contacts.iter()
            .map(|c| {
                let mut parts = vec![c.name.clone()];
                if let Some(t) = &c.title { parts.push(t.clone()); }
                parts.join(" ")
            })
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "{} {} {} {} {}",
            self.name,
            self.normalized_address.as_deref().unwrap_or(&self.address),
            self.notes.as_deref().unwrap_or(""),
            self.tags.join(" "),
            contact_text,
        )
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateFacilityRequest {
    pub name: String,
    pub address: String,
    #[serde(default)]
    pub contacts: Vec<FacilityContact>,
    pub notes: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateFacilityRequest {
    pub name: Option<String>,
    pub address: Option<String>,
    pub contacts: Option<Vec<FacilityContact>>,
    pub notes: Option<String>,
    pub tags: Option<Vec<String>>,
    pub blob_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FacilityListItem {
    pub id: Uuid,
    pub owner_id: i64,
    pub name: String,
    pub address: String,
    pub normalized_address: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub geocode_status: GeocodeStatus,
    pub contacts: Vec<FacilityContact>,
    pub notes: Option<String>,
    pub tags: Vec<String>,
    pub blob_ids: Vec<Uuid>,
    pub avg_dwell_minutes: Option<f64>,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

impl From<FacilityRecord> for FacilityListItem {
    fn from(r: FacilityRecord) -> Self {
        Self {
            id: r.id, owner_id: r.owner_id, name: r.name,
            address: r.address, normalized_address: r.normalized_address,
            lat: r.lat, lng: r.lng, geocode_status: r.geocode_status,
            contacts: r.contacts, notes: r.notes, tags: r.tags,
            blob_ids: r.blob_ids, avg_dwell_minutes: r.avg_dwell_minutes,
            created_at: r.created_at, score: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct FacilityListResponse {
    pub total: usize,
    pub items: Vec<FacilityListItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FacilityCandidate {
    pub id: Uuid,
    pub name: String,
    pub address: String,
    pub normalized_address: Option<String>,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FacilityResolutionResponse {
    pub facility_resolution_required: bool,
    pub candidates: Vec<FacilityCandidate>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_geocode_status_roundtrip() {
        for s in ["pending", "ready", "failed"] {
            let status: GeocodeStatus = s.parse().unwrap();
            assert_eq!(status.as_str(), s);
        }
    }

    #[test]
    fn test_facility_record_embedding_skipped_in_json() {
        let r = FacilityRecord {
            id: uuid::Uuid::new_v4(), owner_id: 0,
            name: "Test Facility".into(), address: "Memphis, TN".into(),
            normalized_address: None, lat: None, lng: None,
            geocode_status: GeocodeStatus::Pending,
            contacts: vec![], notes: None, tags: vec![],
            blob_ids: vec![], avg_dwell_minutes: None,
            dwell_sample_count: 0, embedding: Some(vec![0.1, 0.2]),
            created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(),
        };
        let json = serde_json::to_value(&r).unwrap();
        assert!(json.get("embedding").is_none());
    }

    #[test]
    fn test_facility_embedding_text_includes_contacts() {
        let r = FacilityRecord {
            id: uuid::Uuid::new_v4(), owner_id: 0,
            name: "ABC Warehouse".into(), address: "Memphis, TN".into(),
            normalized_address: Some("315 Industrial Blvd, Memphis, TN 38118".into()),
            lat: None, lng: None, geocode_status: GeocodeStatus::Pending,
            contacts: vec![FacilityContact {
                name: "Jane Smith".into(), title: Some("Dock Manager".into()),
                phone: None, email: None, notes: None,
            }],
            notes: Some("call ahead".into()), tags: vec!["cold".into()],
            blob_ids: vec![], avg_dwell_minutes: None, dwell_sample_count: 0,
            embedding: None,
            created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(),
        };
        let text = r.embedding_text();
        assert!(text.contains("ABC Warehouse"));
        assert!(text.contains("Jane Smith"));
        assert!(text.contains("Dock Manager"));
    }
}
