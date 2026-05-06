// src/models/load.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StopType { Pickup, Delivery }

impl StopType {
    pub fn as_str(&self) -> &'static str {
        match self { Self::Pickup => "pickup", Self::Delivery => "delivery" }
    }
}

impl std::str::FromStr for StopType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pickup" => Ok(Self::Pickup),
            "delivery" => Ok(Self::Delivery),
            other => Err(format!("unknown stop type: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServiceType { PreLoaded, LiveLoad, LiveUnload, DropAndHook, Relay }

impl ServiceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PreLoaded => "pre_loaded", Self::LiveLoad => "live_load",
            Self::LiveUnload => "live_unload", Self::DropAndHook => "drop_and_hook",
            Self::Relay => "relay",
        }
    }

    pub fn is_valid_for(&self, stop_type: &StopType) -> bool {
        matches!((self, stop_type),
            (Self::PreLoaded | Self::LiveLoad, StopType::Pickup)
            | (Self::LiveUnload | Self::DropAndHook, StopType::Delivery)
            | (Self::Relay, _)
        )
    }
}

impl std::str::FromStr for ServiceType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pre_loaded" => Ok(Self::PreLoaded), "live_load" => Ok(Self::LiveLoad),
            "live_unload" => Ok(Self::LiveUnload), "drop_and_hook" => Ok(Self::DropAndHook),
            "relay" => Ok(Self::Relay),
            other => Err(format!("unknown service type: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoadStatus {
    Planned, Dispatched, InTransit, Delivered, Invoiced, Settled, Cancelled,
}

impl LoadStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Planned => "planned", Self::Dispatched => "dispatched",
            Self::InTransit => "in_transit", Self::Delivered => "delivered",
            Self::Invoiced => "invoiced", Self::Settled => "settled",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn can_transition_to(&self, next: &LoadStatus) -> bool {
        match (self, next) {
            (Self::Planned, Self::Dispatched) => true,
            (Self::Dispatched, Self::InTransit) => true,
            (Self::Dispatched | Self::InTransit, Self::Delivered) => true,
            (Self::Delivered, Self::Invoiced) => true,
            (Self::Invoiced, Self::Settled) => true,
            // Cancel only from pre-delivery states; delivered/invoiced/settled are terminal
            (Self::Planned | Self::Dispatched | Self::InTransit, Self::Cancelled) => true,
            _ => false,
        }
    }
}

impl std::str::FromStr for LoadStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "planned" => Ok(Self::Planned), "dispatched" => Ok(Self::Dispatched),
            "in_transit" => Ok(Self::InTransit), "delivered" => Ok(Self::Delivered),
            "invoiced" => Ok(Self::Invoiced), "settled" => Ok(Self::Settled),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(format!("unknown load status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RateLineItem {
    pub description: String,
    pub amount_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Stop {
    pub sequence: u32,
    pub stop_type: StopType,
    pub service_type: ServiceType,
    pub facility_id: Uuid,
    pub scheduled_arrive: String,
    pub notes: Option<String>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StopInput {
    pub sequence: u32,
    pub stop_type: StopType,
    pub service_type: ServiceType,
    pub facility_id: Option<Uuid>,
    pub facility_name: Option<String>,
    pub address: Option<String>,
    #[serde(default)]
    pub force_new_facility: bool,
    pub scheduled_arrive: String,
    pub notes: Option<String>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct StopResponse {
    pub sequence: u32,
    pub stop_type: StopType,
    pub service_type: ServiceType,
    pub facility_id: Uuid,
    pub facility_name: String,
    pub address: String,
    pub normalized_address: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub scheduled_arrive: String,
    pub notes: Option<String>,
    pub blob_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LoadRecord {
    pub id: Uuid,
    pub load_number: String,
    pub owner_id: i64,
    pub status: LoadStatus,
    pub customer_name: String,
    pub customer_ref: Option<String>,
    pub stops: Vec<Stop>,
    pub rate_items: Vec<RateLineItem>,
    pub commodity: Option<String>,
    pub weight_lbs: Option<f64>,
    pub miles: Option<f64>,
    pub notes: Option<String>,
    pub tags: Vec<String>,
    pub blob_ids: Vec<Uuid>,
    pub invoice_number: Option<String>,
    pub invoice_date: Option<String>,
    pub cancellation_reason: Option<String>,
    #[serde(skip)]
    #[schema(skip)]
    pub embedding: Option<Vec<f32>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl LoadRecord {
    pub fn total_rate_usd(&self) -> f64 {
        self.rate_items.iter().map(|r| r.amount_usd).sum()
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateLoadRequest {
    pub load_number: Option<String>,
    pub customer_name: String,
    pub customer_ref: Option<String>,
    pub stops: Vec<StopInput>,
    #[serde(default)]
    pub rate_items: Vec<RateLineItem>,
    pub commodity: Option<String>,
    pub weight_lbs: Option<f64>,
    pub miles: Option<f64>,
    pub notes: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateLoadRequest {
    pub customer_name: Option<String>,
    pub customer_ref: Option<String>,
    pub stops: Option<Vec<StopInput>>,
    pub rate_items: Option<Vec<RateLineItem>>,
    pub commodity: Option<String>,
    pub weight_lbs: Option<f64>,
    pub miles: Option<f64>,
    pub notes: Option<String>,
    pub tags: Option<Vec<String>>,
    pub blob_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct InvoiceActionRequest {
    pub invoice_number: Option<String>,
    pub invoice_date: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CancelActionRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct LoadListItem {
    pub id: Uuid,
    pub load_number: String,
    pub status: LoadStatus,
    pub customer_name: String,
    pub customer_ref: Option<String>,
    pub stops: Vec<Stop>,
    pub rate_items: Vec<RateLineItem>,
    pub total_rate_usd: f64,
    pub commodity: Option<String>,
    pub weight_lbs: Option<f64>,
    pub miles: Option<f64>,
    pub notes: Option<String>,
    pub tags: Vec<String>,
    pub blob_ids: Vec<Uuid>,
    pub invoice_number: Option<String>,
    pub invoice_date: Option<String>,
    pub cancellation_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

impl From<LoadRecord> for LoadListItem {
    fn from(r: LoadRecord) -> Self {
        let total = r.total_rate_usd();
        Self {
            id: r.id, load_number: r.load_number, status: r.status,
            customer_name: r.customer_name, customer_ref: r.customer_ref,
            stops: r.stops, rate_items: r.rate_items, total_rate_usd: total,
            commodity: r.commodity, weight_lbs: r.weight_lbs, miles: r.miles,
            notes: r.notes, tags: r.tags, blob_ids: r.blob_ids,
            invoice_number: r.invoice_number, invoice_date: r.invoice_date,
            cancellation_reason: r.cancellation_reason,
            created_at: r.created_at, score: None,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoadListResponse {
    pub total: usize,
    pub items: Vec<LoadListItem>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct LoadDetailResponse {
    pub id: Uuid,
    pub load_number: String,
    pub status: LoadStatus,
    pub customer_name: String,
    pub customer_ref: Option<String>,
    pub stops: Vec<StopResponse>,
    pub rate_items: Vec<RateLineItem>,
    pub total_rate_usd: f64,
    pub commodity: Option<String>,
    pub weight_lbs: Option<f64>,
    pub miles: Option<f64>,
    pub notes: Option<String>,
    pub tags: Vec<String>,
    pub blob_ids: Vec<Uuid>,
    pub invoice_number: Option<String>,
    pub invoice_date: Option<String>,
    pub cancellation_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stop_type_roundtrip() {
        for s in ["pickup", "delivery"] {
            let t: StopType = s.parse().unwrap();
            assert_eq!(t.as_str(), s);
        }
    }

    #[test]
    fn test_service_type_roundtrip() {
        for s in ["pre_loaded", "live_load", "live_unload", "drop_and_hook", "relay"] {
            let t: ServiceType = s.parse().unwrap();
            assert_eq!(t.as_str(), s);
        }
    }

    #[test]
    fn test_service_type_valid_for_stop() {
        assert!(ServiceType::PreLoaded.is_valid_for(&StopType::Pickup));
        assert!(ServiceType::LiveLoad.is_valid_for(&StopType::Pickup));
        assert!(!ServiceType::LiveUnload.is_valid_for(&StopType::Pickup));
        assert!(!ServiceType::DropAndHook.is_valid_for(&StopType::Pickup));
        assert!(ServiceType::Relay.is_valid_for(&StopType::Pickup));
        assert!(ServiceType::Relay.is_valid_for(&StopType::Delivery));
        assert!(ServiceType::LiveUnload.is_valid_for(&StopType::Delivery));
        assert!(ServiceType::DropAndHook.is_valid_for(&StopType::Delivery));
        assert!(!ServiceType::PreLoaded.is_valid_for(&StopType::Delivery));
    }

    #[test]
    fn test_load_status_roundtrip() {
        for s in ["planned","dispatched","in_transit","delivered","invoiced","settled","cancelled"] {
            let st: LoadStatus = s.parse().unwrap();
            assert_eq!(st.as_str(), s);
        }
    }

    #[test]
    fn test_load_status_valid_transitions() {
        assert!(LoadStatus::Planned.can_transition_to(&LoadStatus::Dispatched));
        assert!(LoadStatus::Dispatched.can_transition_to(&LoadStatus::InTransit));
        assert!(LoadStatus::Dispatched.can_transition_to(&LoadStatus::Delivered));
        assert!(LoadStatus::InTransit.can_transition_to(&LoadStatus::Delivered));
        assert!(LoadStatus::Delivered.can_transition_to(&LoadStatus::Invoiced));
        assert!(LoadStatus::Invoiced.can_transition_to(&LoadStatus::Settled));
        assert!(LoadStatus::Planned.can_transition_to(&LoadStatus::Cancelled));
        assert!(LoadStatus::Dispatched.can_transition_to(&LoadStatus::Cancelled));
        assert!(LoadStatus::InTransit.can_transition_to(&LoadStatus::Cancelled));
        assert!(!LoadStatus::Delivered.can_transition_to(&LoadStatus::Cancelled));
        assert!(!LoadStatus::Invoiced.can_transition_to(&LoadStatus::Cancelled));
        assert!(!LoadStatus::Settled.can_transition_to(&LoadStatus::Dispatched));
        assert!(!LoadStatus::Cancelled.can_transition_to(&LoadStatus::Planned));
        assert!(!LoadStatus::Planned.can_transition_to(&LoadStatus::Delivered));
    }

    #[test]
    fn test_total_rate_usd_sums_including_negatives() {
        let record = LoadRecord {
            id: uuid::Uuid::new_v4(), load_number: "LD-2026-0001".into(),
            owner_id: 0, status: LoadStatus::Planned,
            customer_name: "ACME".into(), customer_ref: None,
            stops: vec![], rate_items: vec![
                RateLineItem { description: "Line Haul".into(), amount_usd: 1800.0 },
                RateLineItem { description: "Fuel Surcharge".into(), amount_usd: 210.0 },
                RateLineItem { description: "Short Pay".into(), amount_usd: -50.0 },
            ],
            commodity: None, weight_lbs: None, miles: None, notes: None,
            tags: vec![], blob_ids: vec![],
            invoice_number: None, invoice_date: None, cancellation_reason: None,
            embedding: None,
            created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(),
        };
        assert!((record.total_rate_usd() - 1960.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_load_record_embedding_skipped_in_json() {
        let r = LoadRecord {
            id: uuid::Uuid::new_v4(), load_number: "LD-2026-0001".into(),
            owner_id: 0, status: LoadStatus::Planned,
            customer_name: "ACME".into(), customer_ref: None,
            stops: vec![], rate_items: vec![], commodity: None,
            weight_lbs: None, miles: None, notes: None, tags: vec![],
            blob_ids: vec![], invoice_number: None, invoice_date: None,
            cancellation_reason: None, embedding: Some(vec![0.1]),
            created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(),
        };
        let json = serde_json::to_value(&r).unwrap();
        assert!(json.get("embedding").is_none());
    }
}
