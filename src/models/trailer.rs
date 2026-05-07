// src/models/trailer.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrailerOwner {
    Fleet,
    Carrier,
    Customer,
    Other,
}

impl TrailerOwner {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fleet => "fleet",
            Self::Carrier => "carrier",
            Self::Customer => "customer",
            Self::Other => "other",
        }
    }
}

impl std::str::FromStr for TrailerOwner {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "fleet" => Ok(Self::Fleet),
            "carrier" => Ok(Self::Carrier),
            "customer" => Ok(Self::Customer),
            "other" => Ok(Self::Other),
            other => Err(format!("unknown trailer owner: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrailerStatus {
    Available,
    Assigned,
    Dispatched,
    OutOfService,
    Inactive,
}

impl TrailerStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Assigned => "assigned",
            Self::Dispatched => "dispatched",
            Self::OutOfService => "out_of_service",
            Self::Inactive => "inactive",
        }
    }

    pub fn can_transition_to(&self, next: &TrailerStatus) -> bool {
        match (self, next) {
            (Self::Available, Self::Assigned) => true,
            (Self::Assigned, Self::Dispatched) => true,
            (Self::Dispatched, Self::Available) => true,
            (_, Self::OutOfService) => true,
            (Self::OutOfService, Self::Available) => true,
            (_, Self::Inactive) => true,
            _ => false,
        }
    }
}

impl std::str::FromStr for TrailerStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "available" => Ok(Self::Available),
            "assigned" => Ok(Self::Assigned),
            "dispatched" => Ok(Self::Dispatched),
            "out_of_service" => Ok(Self::OutOfService),
            "inactive" => Ok(Self::Inactive),
            other => Err(format!("unknown trailer status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TrailerRecord {
    pub id: Uuid,
    pub unit_number: String,
    pub owner: TrailerOwner,
    pub owner_name: Option<String>,
    pub year: Option<i32>,
    pub make: Option<String>,
    pub trailer_type: Option<String>,
    pub length_ft: Option<f64>,
    pub vin: Option<String>,
    pub plate: Option<String>,
    pub plate_state: Option<String>,
    pub status: TrailerStatus,
    pub notes: Option<String>,
    #[serde(skip)]
    #[schema(skip)]
    pub embedding: Option<Vec<f32>>,
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TrailerRecord {
    pub fn embedding_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.unit_number,
            self.owner_name.as_deref().unwrap_or(""),
            self.trailer_type.as_deref().unwrap_or(""),
            self.notes.as_deref().unwrap_or("")
        )
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTrailerRequest {
    pub unit_number: String,
    pub owner: TrailerOwner,
    pub owner_name: Option<String>,
    pub year: Option<i32>,
    pub make: Option<String>,
    pub trailer_type: Option<String>,
    pub length_ft: Option<f64>,
    pub vin: Option<String>,
    pub plate: Option<String>,
    pub plate_state: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateTrailerRequest {
    pub unit_number: Option<String>,
    pub owner: Option<TrailerOwner>,
    pub owner_name: Option<String>,
    pub year: Option<i32>,
    pub make: Option<String>,
    pub trailer_type: Option<String>,
    pub length_ft: Option<f64>,
    pub vin: Option<String>,
    pub plate: Option<String>,
    pub plate_state: Option<String>,
    pub notes: Option<String>,
    pub status: Option<TrailerStatus>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TrailerListItem {
    pub id: Uuid,
    pub unit_number: String,
    pub owner: TrailerOwner,
    pub owner_name: Option<String>,
    pub year: Option<i32>,
    pub make: Option<String>,
    pub trailer_type: Option<String>,
    pub length_ft: Option<f64>,
    pub vin: Option<String>,
    pub plate: Option<String>,
    pub plate_state: Option<String>,
    pub status: TrailerStatus,
    pub notes: Option<String>,
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

impl From<TrailerRecord> for TrailerListItem {
    fn from(r: TrailerRecord) -> Self {
        Self {
            id: r.id,
            unit_number: r.unit_number,
            owner: r.owner,
            owner_name: r.owner_name,
            year: r.year,
            make: r.make,
            trailer_type: r.trailer_type,
            length_ft: r.length_ft,
            vin: r.vin,
            plate: r.plate,
            plate_state: r.plate_state,
            status: r.status,
            notes: r.notes,
            owner_id: r.owner_id,
            created_at: r.created_at,
            score: None,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrailerListResponse {
    pub returned: usize,
    pub items: Vec<TrailerListItem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trailer_owner_roundtrip() {
        for s in ["fleet", "carrier", "customer", "other"] {
            let o: TrailerOwner = s.parse().unwrap();
            assert_eq!(o.as_str(), s);
        }
    }

    #[test]
    fn test_trailer_status_roundtrip() {
        for s in ["available", "assigned", "dispatched", "out_of_service", "inactive"] {
            let st: TrailerStatus = s.parse().unwrap();
            assert_eq!(st.as_str(), s);
        }
    }

    #[test]
    fn test_trailer_status_transitions() {
        assert!(TrailerStatus::Available.can_transition_to(&TrailerStatus::Assigned));
        assert!(TrailerStatus::Assigned.can_transition_to(&TrailerStatus::Dispatched));
        assert!(TrailerStatus::Dispatched.can_transition_to(&TrailerStatus::Available));
        assert!(TrailerStatus::Available.can_transition_to(&TrailerStatus::OutOfService));
        assert!(TrailerStatus::OutOfService.can_transition_to(&TrailerStatus::Available));
        assert!(TrailerStatus::Available.can_transition_to(&TrailerStatus::Inactive));
        assert!(!TrailerStatus::Inactive.can_transition_to(&TrailerStatus::Available));
    }

    #[test]
    fn test_trailer_record_embedding_skipped_in_json() {
        let now = chrono::Utc::now();
        let r = TrailerRecord {
            id: Uuid::new_v4(),
            unit_number: "TR-100".into(),
            owner: TrailerOwner::Fleet,
            owner_name: None,
            year: Some(2021),
            make: Some("Wabash".into()),
            trailer_type: Some("dry_van".into()),
            length_ft: Some(53.0),
            vin: None,
            plate: None,
            plate_state: None,
            status: TrailerStatus::Available,
            notes: None,
            embedding: Some(vec![0.1]),
            owner_id: 0,
            created_at: now,
            updated_at: now,
        };
        let json = serde_json::to_value(&r).unwrap();
        assert!(json.get("embedding").is_none());
    }
}
