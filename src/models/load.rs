// src/models/load.rs
use chrono::{DateTime, NaiveDateTime, Utc};
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
    Planned, Assigned, Dispatched, InTransit, Delivered, Invoiced, Settled, Cancelled,
}

impl LoadStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Planned => "planned", Self::Assigned => "assigned",
            Self::Dispatched => "dispatched", Self::InTransit => "in_transit",
            Self::Delivered => "delivered", Self::Invoiced => "invoiced",
            Self::Settled => "settled", Self::Cancelled => "cancelled",
        }
    }

    pub fn can_transition_to(&self, next: &LoadStatus) -> bool {
        match (self, next) {
            (Self::Planned, Self::Assigned) => true,
            (Self::Assigned, Self::Dispatched) => true,
            (Self::Dispatched, Self::InTransit) => true,
            (Self::Dispatched | Self::InTransit, Self::Delivered) => true,
            (Self::Delivered, Self::Invoiced) => true,
            (Self::Invoiced, Self::Settled) => true,
            // Cancel only from pre-delivery states; delivered/invoiced/settled are terminal
            (Self::Planned | Self::Assigned | Self::Dispatched | Self::InTransit, Self::Cancelled) => true,
            _ => false,
        }
    }
}

impl std::str::FromStr for LoadStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "planned" => Ok(Self::Planned), "assigned" => Ok(Self::Assigned),
            "dispatched" => Ok(Self::Dispatched), "in_transit" => Ok(Self::InTransit),
            "delivered" => Ok(Self::Delivered), "invoiced" => Ok(Self::Invoiced),
            "settled" => Ok(Self::Settled), "cancelled" => Ok(Self::Cancelled),
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
    #[serde(default)]
    pub scheduled_arrive_end: Option<String>,
    #[serde(default)]
    pub actual_arrive: Option<String>,
    #[serde(default)]
    pub actual_depart: Option<String>,
    #[serde(default)]
    pub expected_dwell_minutes: Option<u32>,
    #[serde(default)]
    pub detention_free_minutes: Option<u32>,
    #[serde(default)]
    pub detention_grace_minutes: Option<u32>,
    pub notes: Option<String>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
    #[serde(default)]
    pub timezone: Option<String>,
}

impl Stop {
    pub fn detention_eligible(&self) -> Option<bool> {
        let scheduled = parse_stop_time(&self.scheduled_arrive, self.timezone.as_deref())?;
        let actual_arrive = parse_stop_time(self.actual_arrive.as_deref()?, self.timezone.as_deref())?;
        let actual_depart = parse_stop_time(self.actual_depart.as_deref()?, self.timezone.as_deref())?;
        let free_mins = self.detention_free_minutes.unwrap_or(120) as i64;
        let grace_mins = self.detention_grace_minutes.unwrap_or(15) as i64;
        let eligible = if self.scheduled_arrive_end.is_some() {
            actual_depart > actual_arrive + chrono::Duration::minutes(free_mins)
        } else {
            actual_arrive <= scheduled + chrono::Duration::minutes(grace_mins)
        };
        Some(eligible)
    }
}

fn parse_stop_time(s: &str, tz: Option<&str>) -> Option<DateTime<Utc>> {
    use chrono::TimeZone as _;
    match tz {
        Some(tz_str) => {
            let tz: chrono_tz::Tz = tz_str.parse().ok()?;
            let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok()?;
            tz.from_local_datetime(&naive).single().map(|dt: chrono::DateTime<chrono_tz::Tz>| dt.with_timezone(&Utc))
        }
        None => s.parse::<DateTime<Utc>>().ok(),
    }
}

/// Validates that `s` is a naive local datetime parseable in `tz_str`, with no UTC offset.
/// Returns 422 if the format is wrong or the time is DST-ambiguous/nonexistent.
/// Call only when the stop has a timezone; skip for legacy stops (timezone: None).
pub fn validate_stop_time_str(s: &str, tz_str: &str, field: &str) -> Result<(), crate::error::AppError> {
    use chrono::TimeZone as _;
    let tz: chrono_tz::Tz = tz_str.parse()
        .map_err(|_| crate::error::AppError::Internal(format!("invalid timezone stored on stop: {tz_str}")))?;
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
        .map_err(|_| crate::error::AppError::UnprocessableEntity(format!(
            "{field}: must be a naive local datetime (e.g. \"2026-05-10T09:15:00\") — omit timezone offset"
        )))?;
    tz.from_local_datetime(&naive).single()
        .ok_or_else(|| crate::error::AppError::UnprocessableEntity(format!(
            "{field}: time is ambiguous or nonexistent in {tz_str} (DST boundary)"
        )))?;
    Ok(())
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
    #[serde(default)]
    pub scheduled_arrive_end: Option<String>,
    #[serde(default)]
    pub actual_arrive: Option<String>,
    #[serde(default)]
    pub actual_depart: Option<String>,
    #[serde(default)]
    pub expected_dwell_minutes: Option<u32>,
    #[serde(default)]
    pub detention_free_minutes: Option<u32>,
    #[serde(default)]
    pub detention_grace_minutes: Option<u32>,
    pub notes: Option<String>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
    pub timezone: String,
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
    pub scheduled_arrive_end: Option<String>,
    pub actual_arrive: Option<String>,
    pub actual_depart: Option<String>,
    pub expected_dwell_minutes: Option<u32>,
    pub detention_free_minutes: Option<u32>,
    pub detention_grace_minutes: Option<u32>,
    pub notes: Option<String>,
    pub blob_ids: Vec<Uuid>,
    pub timezone: Option<String>,
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
    /// Number of items returned. For list mode equals total matching count; for search mode equals items in this response.
    pub returned: usize,
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
        for s in ["planned","assigned","dispatched","in_transit","delivered","invoiced","settled","cancelled"] {
            let st: LoadStatus = s.parse().unwrap();
            assert_eq!(st.as_str(), s);
        }
    }

    #[test]
    fn test_load_status_valid_transitions() {
        assert!(LoadStatus::Planned.can_transition_to(&LoadStatus::Assigned));
        assert!(LoadStatus::Assigned.can_transition_to(&LoadStatus::Dispatched));
        assert!(LoadStatus::Dispatched.can_transition_to(&LoadStatus::InTransit));
        assert!(LoadStatus::Dispatched.can_transition_to(&LoadStatus::Delivered));
        assert!(LoadStatus::InTransit.can_transition_to(&LoadStatus::Delivered));
        assert!(LoadStatus::Delivered.can_transition_to(&LoadStatus::Invoiced));
        assert!(LoadStatus::Invoiced.can_transition_to(&LoadStatus::Settled));
        assert!(LoadStatus::Planned.can_transition_to(&LoadStatus::Cancelled));
        assert!(LoadStatus::Assigned.can_transition_to(&LoadStatus::Cancelled));
        assert!(LoadStatus::Dispatched.can_transition_to(&LoadStatus::Cancelled));
        assert!(LoadStatus::InTransit.can_transition_to(&LoadStatus::Cancelled));
        assert!(!LoadStatus::Delivered.can_transition_to(&LoadStatus::Cancelled));
        assert!(!LoadStatus::Invoiced.can_transition_to(&LoadStatus::Cancelled));
        assert!(!LoadStatus::Settled.can_transition_to(&LoadStatus::Dispatched));
        assert!(!LoadStatus::Cancelled.can_transition_to(&LoadStatus::Planned));
        assert!(!LoadStatus::Planned.can_transition_to(&LoadStatus::Delivered));
        assert!(!LoadStatus::Planned.can_transition_to(&LoadStatus::Dispatched));
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

    #[test]
    fn test_stop_deserializes_without_new_fields() {
        let json = r#"{
            "sequence": 0,
            "stop_type": "pickup",
            "service_type": "live_load",
            "facility_id": "00000000-0000-0000-0000-000000000001",
            "scheduled_arrive": "2026-05-10T08:00:00Z",
            "notes": null,
            "blob_ids": []
        }"#;
        let stop: Stop = serde_json::from_str(json).unwrap();
        assert_eq!(stop.sequence, 0);
        assert!(stop.scheduled_arrive_end.is_none());
        assert!(stop.actual_arrive.is_none());
        assert!(stop.actual_depart.is_none());
        assert!(stop.expected_dwell_minutes.is_none());
        assert!(stop.detention_free_minutes.is_none());
        assert!(stop.detention_grace_minutes.is_none());
        assert!(stop.timezone.is_none(), "legacy stops without timezone field must deserialize as None");
    }

    fn legacy_stop(
        scheduled_arrive: &str,
        scheduled_arrive_end: Option<&str>,
        actual_arrive: Option<&str>,
        actual_depart: Option<&str>,
    ) -> Stop {
        Stop {
            sequence: 0,
            stop_type: StopType::Delivery,
            service_type: ServiceType::LiveUnload,
            facility_id: Uuid::new_v4(),
            scheduled_arrive: scheduled_arrive.into(),
            scheduled_arrive_end: scheduled_arrive_end.map(Into::into),
            actual_arrive: actual_arrive.map(Into::into),
            actual_depart: actual_depart.map(Into::into),
            expected_dwell_minutes: None,
            detention_free_minutes: None,
            detention_grace_minutes: None,
            notes: None,
            blob_ids: vec![],
            timezone: None,
        }
    }

    fn tz_stop(
        scheduled_arrive: &str,
        scheduled_arrive_end: Option<&str>,
        actual_arrive: Option<&str>,
        actual_depart: Option<&str>,
        tz: &str,
    ) -> Stop {
        Stop {
            timezone: Some(tz.into()),
            scheduled_arrive: scheduled_arrive.into(),
            scheduled_arrive_end: scheduled_arrive_end.map(Into::into),
            actual_arrive: actual_arrive.map(Into::into),
            actual_depart: actual_depart.map(Into::into),
            ..legacy_stop(scheduled_arrive, scheduled_arrive_end, actual_arrive, actual_depart)
        }
    }

    #[test]
    fn test_detention_eligible_fcfs() {
        // FCFS: scheduled_arrive_end set → eligible if actual_depart > actual_arrive + free_mins (default 120)
        let stop = legacy_stop(
            "2026-05-10T08:00:00Z",
            Some("2026-05-10T12:00:00Z"),
            Some("2026-05-10T09:00:00Z"),
            Some("2026-05-10T12:01:00Z"), // 181 min dwell > 120
        );
        assert_eq!(stop.detention_eligible(), Some(true));
    }

    #[test]
    fn test_detention_eligible_strict_on_time() {
        // Strict: arrival within grace window → eligible
        let stop = legacy_stop(
            "2026-05-10T08:00:00Z",
            None,
            Some("2026-05-10T08:10:00Z"), // 10 min late ≤ 15 grace
            Some("2026-05-10T10:30:00Z"),
        );
        assert_eq!(stop.detention_eligible(), Some(true));
    }

    #[test]
    fn test_detention_eligible_strict_late_ineligible() {
        // Strict: late arrival beyond grace → not eligible
        let stop = legacy_stop(
            "2026-05-10T08:00:00Z",
            None,
            Some("2026-05-10T08:20:00Z"), // 20 min late > 15 grace
            Some("2026-05-10T10:30:00Z"),
        );
        assert_eq!(stop.detention_eligible(), Some(false));
    }

    #[test]
    fn test_detention_eligible_strict_early_arrival() {
        // Strict: early arrival is always on-time (negative lateness passes)
        let stop = Stop {
            stop_type: StopType::Pickup,
            service_type: ServiceType::LiveLoad,
            ..legacy_stop(
                "2026-05-10T08:00:00Z",
                None,
                Some("2026-05-10T07:45:00Z"), // 15 min early
                Some("2026-05-10T10:30:00Z"),
            )
        };
        assert_eq!(stop.detention_eligible(), Some(true));
    }

    #[test]
    fn test_detention_eligible_with_timezone_strict_on_time() {
        // Timezone-aware: naive local times + IANA tz → correct UTC conversion
        let stop = tz_stop(
            "2026-05-10T08:00:00",
            None,
            Some("2026-05-10T08:10:00"), // 10 min late ≤ 15 grace
            Some("2026-05-10T10:30:00"),
            "America/Chicago",
        );
        assert_eq!(stop.detention_eligible(), Some(true));
    }

    #[test]
    fn test_detention_eligible_with_timezone_strict_late_ineligible() {
        let stop = tz_stop(
            "2026-05-10T08:00:00",
            None,
            Some("2026-05-10T08:20:00"), // 20 min late > 15 grace
            Some("2026-05-10T10:30:00"),
            "America/New_York",
        );
        assert_eq!(stop.detention_eligible(), Some(false));
    }

    #[test]
    fn test_detention_eligible_with_timezone_fcfs() {
        let stop = tz_stop(
            "2026-05-10T08:00:00",
            Some("2026-05-10T12:00:00"),
            Some("2026-05-10T09:00:00"),
            Some("2026-05-10T12:01:00"), // 181 min dwell > 120
            "America/Los_Angeles",
        );
        assert_eq!(stop.detention_eligible(), Some(true));
    }

    #[test]
    fn test_detention_eligible_returns_none_for_missing_actuals() {
        let stop = tz_stop("2026-05-10T08:00:00", None, None, None, "America/Chicago");
        assert_eq!(stop.detention_eligible(), None);
    }
}
