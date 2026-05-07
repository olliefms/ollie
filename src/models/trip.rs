// src/models/trip.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TripStopType {
    Origin,
    Fuel,
    Pickup,
    Delivery,
    Relay,
    EmptyMove,
    Maintenance,
    Terminal,
}

impl TripStopType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Origin => "origin",
            Self::Fuel => "fuel",
            Self::Pickup => "pickup",
            Self::Delivery => "delivery",
            Self::Relay => "relay",
            Self::EmptyMove => "empty_move",
            Self::Maintenance => "maintenance",
            Self::Terminal => "terminal",
        }
    }
}

impl std::str::FromStr for TripStopType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "origin" => Ok(Self::Origin),
            "fuel" => Ok(Self::Fuel),
            "pickup" => Ok(Self::Pickup),
            "delivery" => Ok(Self::Delivery),
            "relay" => Ok(Self::Relay),
            "empty_move" => Ok(Self::EmptyMove),
            "maintenance" => Ok(Self::Maintenance),
            "terminal" => Ok(Self::Terminal),
            other => Err(format!("unknown trip stop type: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TripStop {
    pub sequence: u32,
    pub stop_type: TripStopType,
    pub facility_id: Option<Uuid>,
    pub name: Option<String>,
    pub load_stop_index: Option<u32>,
    pub scheduled_arrive: Option<String>,
    pub scheduled_arrive_end: Option<String>,
    pub actual_arrive: Option<String>,
    pub actual_depart: Option<String>,
    pub expected_dwell_minutes: Option<u32>,
    pub detention_free_minutes: Option<u32>,
    pub detention_grace_minutes: Option<u32>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TripStatus {
    Planned,
    Assigned,
    Dispatched,
    InTransit,
    Delivered,
    Cancelled,
}

impl TripStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Assigned => "assigned",
            Self::Dispatched => "dispatched",
            Self::InTransit => "in_transit",
            Self::Delivered => "delivered",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn can_transition_to(&self, next: &TripStatus) -> bool {
        match (self, next) {
            (Self::Planned, Self::Assigned) => true,
            (Self::Assigned, Self::Planned) => true,
            (Self::Assigned, Self::Dispatched) => true,
            (Self::Dispatched, Self::Assigned) => true,
            (Self::Dispatched, Self::InTransit) => true,
            (Self::InTransit, Self::Delivered) => true,
            (Self::Planned | Self::Assigned | Self::Dispatched, Self::Cancelled) => true,
            _ => false,
        }
    }
}

impl std::str::FromStr for TripStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "planned" => Ok(Self::Planned),
            "assigned" => Ok(Self::Assigned),
            "dispatched" => Ok(Self::Dispatched),
            "in_transit" => Ok(Self::InTransit),
            "delivered" => Ok(Self::Delivered),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(format!("unknown trip status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TripRecord {
    pub id: Uuid,
    pub trip_number: String,
    pub load_id: Option<Uuid>,
    pub sequence: u32,
    pub driver_id: Option<Uuid>,
    pub truck_id: Option<Uuid>,
    pub trailer_ids: Vec<Uuid>,
    pub status: TripStatus,
    pub stops: Vec<TripStop>,
    pub notes: Option<String>,
    #[serde(skip)]
    #[schema(skip)]
    pub embedding: Option<Vec<f32>>,
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TripRecord {
    pub fn embedding_text(&self) -> String {
        let stop_names = self.stops.iter()
            .filter_map(|s| s.name.as_deref())
            .collect::<Vec<_>>().join(" ");
        format!("{} {} {}", self.trip_number, stop_names, self.notes.as_deref().unwrap_or(""))
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTripRequest {
    pub trip_number: Option<String>,
    pub load_id: Option<Uuid>,
    pub sequence: Option<u32>,
    pub driver_id: Option<Uuid>,
    pub truck_id: Option<Uuid>,
    #[serde(default)]
    pub trailer_ids: Vec<Uuid>,
    #[serde(default)]
    pub stops: Vec<TripStop>,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateTripRequest {
    pub load_id: Option<Uuid>,
    pub sequence: Option<u32>,
    pub stops: Option<Vec<TripStop>>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TripListItem {
    pub id: Uuid,
    pub trip_number: String,
    pub load_id: Option<Uuid>,
    pub sequence: u32,
    pub driver_id: Option<Uuid>,
    pub truck_id: Option<Uuid>,
    pub trailer_ids: Vec<Uuid>,
    pub status: TripStatus,
    pub stops: Vec<TripStop>,
    pub notes: Option<String>,
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

impl From<TripRecord> for TripListItem {
    fn from(r: TripRecord) -> Self {
        Self {
            id: r.id,
            trip_number: r.trip_number,
            load_id: r.load_id,
            sequence: r.sequence,
            driver_id: r.driver_id,
            truck_id: r.truck_id,
            trailer_ids: r.trailer_ids,
            status: r.status,
            stops: r.stops,
            notes: r.notes,
            owner_id: r.owner_id,
            created_at: r.created_at,
            updated_at: r.updated_at,
            score: None,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TripListResponse {
    pub returned: usize,
    pub items: Vec<TripListItem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trip_stop_type_roundtrip() {
        for s in ["origin", "fuel", "pickup", "delivery", "relay", "empty_move", "maintenance", "terminal"] {
            let t: TripStopType = s.parse().unwrap();
            assert_eq!(t.as_str(), s);
        }
    }

    #[test]
    fn test_trip_status_roundtrip() {
        for s in ["planned", "assigned", "dispatched", "in_transit", "delivered", "cancelled"] {
            let st: TripStatus = s.parse().unwrap();
            assert_eq!(st.as_str(), s);
        }
    }

    #[test]
    fn test_trip_status_transitions() {
        assert!(TripStatus::Planned.can_transition_to(&TripStatus::Assigned));
        assert!(TripStatus::Assigned.can_transition_to(&TripStatus::Planned));
        assert!(TripStatus::Assigned.can_transition_to(&TripStatus::Dispatched));
        assert!(TripStatus::Dispatched.can_transition_to(&TripStatus::Assigned));
        assert!(TripStatus::Dispatched.can_transition_to(&TripStatus::InTransit));
        assert!(TripStatus::InTransit.can_transition_to(&TripStatus::Delivered));
        assert!(TripStatus::Planned.can_transition_to(&TripStatus::Cancelled));
        assert!(TripStatus::Assigned.can_transition_to(&TripStatus::Cancelled));
        assert!(TripStatus::Dispatched.can_transition_to(&TripStatus::Cancelled));
        assert!(!TripStatus::InTransit.can_transition_to(&TripStatus::Cancelled));
        assert!(!TripStatus::Delivered.can_transition_to(&TripStatus::Cancelled));
        assert!(!TripStatus::Planned.can_transition_to(&TripStatus::Delivered));
        assert!(!TripStatus::Delivered.can_transition_to(&TripStatus::Planned));
    }

    #[test]
    fn test_embedding_text() {
        let now = chrono::Utc::now();
        let r = TripRecord {
            id: Uuid::new_v4(),
            trip_number: "T-2026-0001".into(),
            load_id: None,
            sequence: 0,
            driver_id: None,
            truck_id: None,
            trailer_ids: vec![],
            status: TripStatus::Planned,
            stops: vec![
                TripStop {
                    sequence: 0,
                    stop_type: TripStopType::Pickup,
                    facility_id: None,
                    name: Some("Chicago Hub".into()),
                    load_stop_index: None,
                    scheduled_arrive: None,
                    scheduled_arrive_end: None,
                    actual_arrive: None,
                    actual_depart: None,
                    expected_dwell_minutes: None,
                    detention_free_minutes: None,
                    detention_grace_minutes: None,
                    notes: None,
                },
            ],
            notes: Some("urgent".into()),
            embedding: None,
            owner_id: 0,
            created_at: now,
            updated_at: now,
        };
        let text = r.embedding_text();
        assert!(text.contains("T-2026-0001"));
        assert!(text.contains("Chicago Hub"));
        assert!(text.contains("urgent"));
    }

    #[test]
    fn test_trip_record_embedding_skipped_in_json() {
        let now = chrono::Utc::now();
        let r = TripRecord {
            id: Uuid::new_v4(),
            trip_number: "T-2026-0001".into(),
            load_id: None,
            sequence: 0,
            driver_id: None,
            truck_id: None,
            trailer_ids: vec![],
            status: TripStatus::Planned,
            stops: vec![],
            notes: None,
            embedding: Some(vec![0.1, 0.2]),
            owner_id: 0,
            created_at: now,
            updated_at: now,
        };
        let json = serde_json::to_value(&r).unwrap();
        assert!(json.get("embedding").is_none());
    }
}
