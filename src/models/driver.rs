// src/models/driver.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use super::double_option;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DriverStatus {
    Available,
    Assigned,
    Dispatched,
    Inactive,
}

impl DriverStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Assigned => "assigned",
            Self::Dispatched => "dispatched",
            Self::Inactive => "inactive",
        }
    }

    /// Returns true if the state machine allows transitioning from self to next.
    /// Note: assigned and dispatched are driven by trip events only; PUT cannot set them.
    pub fn can_transition_to(&self, next: &DriverStatus) -> bool {
        matches!((self, next),
            (Self::Available, Self::Assigned)
            | (Self::Assigned, Self::Dispatched)
            | (Self::Dispatched, Self::Available)
            | (_, Self::Inactive)
        )
    }
}

impl std::str::FromStr for DriverStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "available" => Ok(Self::Available),
            "assigned" => Ok(Self::Assigned),
            "dispatched" => Ok(Self::Dispatched),
            "inactive" => Ok(Self::Inactive),
            other => Err(format!("unknown driver status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DriverRecord {
    pub id: Uuid,
    pub name: String,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub license_number: Option<String>,
    pub license_state: Option<String>,
    pub license_expiry: Option<String>,
    pub status: DriverStatus,
    pub notes: Option<String>,
    #[serde(default)]
    pub current_truck_id: Option<Uuid>,
    #[serde(default)]
    pub current_trailer_ids: Vec<Uuid>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
    #[serde(skip)]
    #[schema(skip)]
    pub embedding: Option<Vec<f32>>,
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// FK to terminals. Optional for backward-compat deserialization,
    /// but always populated after migration (backfilled to the default terminal).
    #[serde(default)]
    pub terminal_id: Option<Uuid>,
    #[serde(default)]
    pub loaded_rate_per_mile: Option<f64>,
    #[serde(default)]
    pub deadhead_rate_per_mile: Option<f64>,
    #[serde(default)]
    pub extra_stop_fee: Option<f64>,
    #[serde(default)]
    pub detention_rate_per_hour: Option<f64>,
    #[serde(default)]
    pub free_dwell_minutes: Option<u32>,
}

impl DriverRecord {
    pub fn embedding_text(&self) -> String {
        format!("{} {}", self.name, self.notes.as_deref().unwrap_or(""))
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateDriverRequest {
    pub name: String,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub license_number: Option<String>,
    pub license_state: Option<String>,
    pub license_expiry: Option<String>,
    pub notes: Option<String>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
    #[serde(default)]
    pub terminal_id: Option<Uuid>,
    #[serde(default)]
    pub loaded_rate_per_mile: Option<f64>,
    #[serde(default)]
    pub deadhead_rate_per_mile: Option<f64>,
    #[serde(default)]
    pub extra_stop_fee: Option<f64>,
    #[serde(default)]
    pub detention_rate_per_hour: Option<f64>,
    #[serde(default)]
    pub free_dwell_minutes: Option<u32>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateDriverRequest {
    pub name: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub license_number: Option<String>,
    pub license_state: Option<String>,
    pub license_expiry: Option<String>,
    pub notes: Option<String>,
    pub blob_ids: Option<Vec<Uuid>>,
    #[serde(default)]
    pub terminal_id: Option<Uuid>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<f64>)]
    pub loaded_rate_per_mile: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<f64>)]
    pub deadhead_rate_per_mile: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<f64>)]
    pub extra_stop_fee: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<f64>)]
    pub detention_rate_per_hour: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<u32>)]
    pub free_dwell_minutes: Option<Option<u32>>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetDriverPinRequest {
    pub pin: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DriverListItem {
    pub id: Uuid,
    pub name: String,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub license_number: Option<String>,
    pub license_state: Option<String>,
    pub license_expiry: Option<String>,
    pub status: DriverStatus,
    pub notes: Option<String>,
    pub blob_ids: Vec<Uuid>,
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    pub terminal_id: Option<Uuid>,
    pub loaded_rate_per_mile: Option<f64>,
    pub deadhead_rate_per_mile: Option<f64>,
    pub extra_stop_fee: Option<f64>,
    pub detention_rate_per_hour: Option<f64>,
    pub free_dwell_minutes: Option<u32>,
}

impl From<DriverRecord> for DriverListItem {
    fn from(r: DriverRecord) -> Self {
        Self {
            id: r.id,
            name: r.name,
            phone: r.phone,
            email: r.email,
            license_number: r.license_number,
            license_state: r.license_state,
            license_expiry: r.license_expiry,
            status: r.status,
            notes: r.notes,
            blob_ids: r.blob_ids,
            owner_id: r.owner_id,
            created_at: r.created_at,
            score: None,
            terminal_id: r.terminal_id,
            loaded_rate_per_mile: r.loaded_rate_per_mile,
            deadhead_rate_per_mile: r.deadhead_rate_per_mile,
            extra_stop_fee: r.extra_stop_fee,
            detention_rate_per_hour: r.detention_rate_per_hour,
            free_dwell_minutes: r.free_dwell_minutes,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DriverListResponse {
    pub returned: usize,
    pub items: Vec<DriverListItem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_driver_status_roundtrip() {
        for s in ["available", "assigned", "dispatched", "inactive"] {
            let st: DriverStatus = s.parse().unwrap();
            assert_eq!(st.as_str(), s);
        }
    }

    #[test]
    fn test_driver_status_transitions() {
        assert!(DriverStatus::Available.can_transition_to(&DriverStatus::Assigned));
        assert!(DriverStatus::Assigned.can_transition_to(&DriverStatus::Dispatched));
        assert!(DriverStatus::Dispatched.can_transition_to(&DriverStatus::Available));
        assert!(DriverStatus::Available.can_transition_to(&DriverStatus::Inactive));
        assert!(DriverStatus::Assigned.can_transition_to(&DriverStatus::Inactive));
        assert!(!DriverStatus::Available.can_transition_to(&DriverStatus::Dispatched));
        assert!(!DriverStatus::Inactive.can_transition_to(&DriverStatus::Available));
    }

    #[test]
    fn test_driver_record_embedding_skipped_in_json() {
        let now = chrono::Utc::now();
        let r = DriverRecord {
            id: Uuid::new_v4(), name: "John Doe".into(),
            phone: None, email: None, license_number: None,
            license_state: None, license_expiry: None,
            status: DriverStatus::Available, notes: None,
            current_truck_id: None, current_trailer_ids: vec![],
            blob_ids: vec![],
            embedding: Some(vec![0.1]),
            owner_id: 0, created_at: now, updated_at: now,
            terminal_id: None, loaded_rate_per_mile: None,
            deadhead_rate_per_mile: None, extra_stop_fee: None,
            detention_rate_per_hour: None, free_dwell_minutes: None,
        };
        let json = serde_json::to_value(&r).unwrap();
        assert!(json.get("embedding").is_none());
    }
}
