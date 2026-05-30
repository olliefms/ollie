// src/models/truck.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TruckStatus {
    Available,
    Assigned,
    Dispatched,
    OutOfService,
    Inactive,
}

impl TruckStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Assigned => "assigned",
            Self::Dispatched => "dispatched",
            Self::OutOfService => "out_of_service",
            Self::Inactive => "inactive",
        }
    }

    pub fn can_transition_to(&self, next: &TruckStatus) -> bool {
        matches!((self, next),
            (Self::Available, Self::Assigned)
            | (Self::Assigned, Self::Dispatched)
            | (Self::Dispatched, Self::Available)
            | (_, Self::OutOfService)
            | (Self::OutOfService, Self::Available)
            | (_, Self::Inactive)
        )
    }
}

impl std::str::FromStr for TruckStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "available" => Ok(Self::Available),
            "assigned" => Ok(Self::Assigned),
            "dispatched" => Ok(Self::Dispatched),
            "out_of_service" => Ok(Self::OutOfService),
            "inactive" => Ok(Self::Inactive),
            other => Err(format!("unknown truck status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TruckRecord {
    pub id: Uuid,
    pub unit_number: String,
    pub year: Option<i32>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub vin: Option<String>,
    pub plate: Option<String>,
    pub plate_state: Option<String>,
    pub status: TruckStatus,
    pub notes: Option<String>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
    #[serde(skip)]
    #[schema(skip)]
    pub embedding: Option<Vec<f32>>,
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TruckRecord {
    pub fn embedding_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.unit_number,
            self.make.as_deref().unwrap_or(""),
            self.model.as_deref().unwrap_or(""),
            self.notes.as_deref().unwrap_or("")
        )
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTruckRequest {
    pub unit_number: String,
    pub year: Option<i32>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub vin: Option<String>,
    pub plate: Option<String>,
    pub plate_state: Option<String>,
    pub notes: Option<String>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateTruckRequest {
    pub unit_number: Option<String>,
    pub year: Option<i32>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub vin: Option<String>,
    pub plate: Option<String>,
    pub plate_state: Option<String>,
    pub notes: Option<String>,
    pub status: Option<TruckStatus>,
    pub blob_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TruckListItem {
    pub id: Uuid,
    pub unit_number: String,
    pub year: Option<i32>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub vin: Option<String>,
    pub plate: Option<String>,
    pub plate_state: Option<String>,
    pub status: TruckStatus,
    pub notes: Option<String>,
    pub blob_ids: Vec<Uuid>,
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

impl From<TruckRecord> for TruckListItem {
    fn from(r: TruckRecord) -> Self {
        Self {
            id: r.id,
            unit_number: r.unit_number,
            year: r.year,
            make: r.make,
            model: r.model,
            vin: r.vin,
            plate: r.plate,
            plate_state: r.plate_state,
            status: r.status,
            notes: r.notes,
            blob_ids: r.blob_ids,
            owner_id: r.owner_id,
            created_at: r.created_at,
            score: None,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TruckListResponse {
    pub returned: usize,
    pub items: Vec<TruckListItem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truck_status_roundtrip() {
        for s in ["available", "assigned", "dispatched", "out_of_service", "inactive"] {
            let st: TruckStatus = s.parse().unwrap();
            assert_eq!(st.as_str(), s);
        }
    }

    #[test]
    fn test_truck_status_transitions() {
        assert!(TruckStatus::Available.can_transition_to(&TruckStatus::Assigned));
        assert!(TruckStatus::Assigned.can_transition_to(&TruckStatus::Dispatched));
        assert!(TruckStatus::Dispatched.can_transition_to(&TruckStatus::Available));
        assert!(TruckStatus::Available.can_transition_to(&TruckStatus::OutOfService));
        assert!(TruckStatus::OutOfService.can_transition_to(&TruckStatus::Available));
        assert!(TruckStatus::Available.can_transition_to(&TruckStatus::Inactive));
        assert!(!TruckStatus::Inactive.can_transition_to(&TruckStatus::Available));
    }

    #[test]
    fn test_truck_record_embedding_skipped_in_json() {
        let now = chrono::Utc::now();
        let r = TruckRecord {
            id: Uuid::new_v4(),
            unit_number: "T-100".into(),
            year: Some(2020),
            make: Some("Kenworth".into()),
            model: None,
            vin: None,
            plate: None,
            plate_state: None,
            status: TruckStatus::Available,
            notes: None,
            blob_ids: vec![],
            embedding: Some(vec![0.1]),
            owner_id: 0,
            created_at: now,
            updated_at: now,
        };
        let json = serde_json::to_value(&r).unwrap();
        assert!(json.get("embedding").is_none());
    }
}
