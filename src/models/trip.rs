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
    /// A non-service stop that affects mileage: a hold awaiting instructions, a
    /// company-mandated routing point, anything the router must pass through
    /// where no freight is serviced.
    Waypoint,
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
            Self::Waypoint => "waypoint",
        }
    }

    /// Whether this stop is freight work the driver is paid an extra-stop fee
    /// for. Non-service stops still route and still accrue detention — they
    /// simply are not an "extra stop".
    pub fn is_service_stop(&self) -> bool {
        matches!(self, Self::Pickup | Self::Delivery | Self::Relay | Self::EmptyMove)
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
            "waypoint" => Ok(Self::Waypoint),
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
    pub address: Option<String>,
    pub load_stop_index: Option<u32>,
    pub scheduled_arrive: Option<String>,
    pub scheduled_arrive_end: Option<String>,
    pub actual_arrive: Option<String>,
    pub actual_depart: Option<String>,
    pub expected_dwell_minutes: Option<u32>,
    pub detention_free_minutes: Option<u32>,
    pub detention_grace_minutes: Option<u32>,
    pub notes: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    /// Response-only: RFC 3339 UTC derived from `actual_arrive` + `timezone`.
    /// Never persisted; populated only by response builders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_arrive_utc: Option<String>,
    /// Response-only: RFC 3339 UTC derived from `actual_depart` + `timezone`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_depart_utc: Option<String>,
}

impl TripStop {
    /// Populate `actual_arrive_utc` and `actual_depart_utc` from the naive +
    /// timezone fields (or from an already-UTC suffix for legacy rows).
    /// Call only on response paths — never on persisted records.
    pub fn fill_utc_fields(&mut self) {
        self.actual_arrive_utc = self.actual_arrive.as_deref()
            .and_then(|s| crate::models::load::naive_local_to_utc(s, self.timezone.as_deref()));
        self.actual_depart_utc = self.actual_depart.as_deref()
            .and_then(|s| crate::models::load::naive_local_to_utc(s, self.timezone.as_deref()));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DeadheadOrigin {
    pub trip_id: Uuid,
    pub facility_name: Option<String>,
    /// `normalized_address` if present, else raw `address`. Free-form single string.
    pub address: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LegMiles {
    /// 0 = deadhead leg (origin → first stop). 1+ = loaded legs between trip stops.
    pub index: u32,
    /// "deadhead" or "loaded"
    pub kind: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub miles: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MileageSummary {
    pub origin: Option<DeadheadOrigin>,
    pub legs: Vec<LegMiles>,
    pub deadhead_miles: Option<f64>,
    pub loaded_miles: Option<f64>,
    pub total_miles: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TripStatus {
    Planned,
    Assigned,
    Dispatched,
    InTransit,
    Delivered,
    Completed,
    Cancelled,
    /// Truck Ordered Not Used: dispatched and rolled, released before loading.
    /// Terminal — real deadhead, no freight.
    Tonu,
}

impl TripStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Assigned => "assigned",
            Self::Dispatched => "dispatched",
            Self::InTransit => "in_transit",
            Self::Delivered => "delivered",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Tonu => "tonu",
        }
    }

    pub fn can_transition_to(&self, next: &TripStatus) -> bool {
        matches!((self, next),
            (Self::Planned, Self::Assigned)
            | (Self::Assigned, Self::Planned)
            | (Self::Assigned, Self::Dispatched)
            | (Self::Dispatched, Self::Assigned)
            | (Self::Dispatched, Self::InTransit)
            | (Self::InTransit, Self::Delivered)
            | (Self::Delivered, Self::Completed)
            | (Self::Planned | Self::Assigned | Self::Dispatched, Self::Cancelled)
            | (Self::Dispatched, Self::Tonu)
        )
    }

    /// The trip's freight is off the truck. `Completed` is the normal end state a
    /// `Delivered` trip moves on to, so anything asking "did this trip finish its
    /// delivery" must accept both.
    pub fn is_delivery_complete(&self) -> bool {
        matches!(self, Self::Delivered | Self::Completed)
    }
}

/// Whether a load's trips collectively say the load has been delivered (#395).
///
/// `Cancelled` **and** `Tonu` trips are dead records for cascade purposes — a
/// superseded pre-assignment must not strand the load, and neither must a leg
/// whose truck was released before loading. A TONU'd leg is superseded by
/// whatever leg is dispatched in its place; its record persists because the
/// driver's deadhead and detention are paid off it, but it never delivers and
/// has no edge out of `Tonu`, so counting it against this check leaves a relay
/// load (deliver to cross-dock, TONU at the dock, re-dispatch and haul out)
/// unable to cascade to `Delivered` *forever* — and therefore unable to
/// invoice, with no manual override and no doctor coverage, since
/// `load_doctor`'s status check is gated behind this same predicate.
///
/// The survivors must all be `Delivered` *or* `Completed`: on a relay load,
/// leg 1 is routinely completed before leg 2 delivers, and an equality check
/// against `Delivered` alone leaves every multi-leg load unable to cascade. A
/// load with no surviving trips has nothing to deliver, so it is `false` rather
/// than the vacuously-true `all()` on an empty iterator — which is what keeps
/// the single-leg case right: a load whose only trip is `Tonu` stays `false`.
pub(crate) fn load_trips_all_delivered(trips: &[TripRecord]) -> bool {
    let mut live = trips.iter()
        .filter(|t| !matches!(t.status, TripStatus::Cancelled | TripStatus::Tonu))
        .peekable();
    live.peek().is_some() && live.all(|t| t.status.is_delivery_complete())
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
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            "tonu" => Ok(Self::Tonu),
            other => Err(format!("unknown trip status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TripRecord {
    pub id: Uuid,
    pub trip_number: String,
    pub load_id: Option<Uuid>,
    pub load_number: Option<String>,
    pub previous_trip_id: Option<Uuid>,
    pub deadhead_miles: Option<f64>,
    pub loaded_miles: Option<f64>,
    pub total_miles: Option<f64>,
    /// Per-segment miles from the single ORS multi-waypoint call. Order:
    /// [deadhead_leg, loaded_leg_1, loaded_leg_2, ...] when origin exists;
    /// [loaded_leg_1, loaded_leg_2, ...] when there's no previous trip.
    /// Empty when ORS routing was unavailable.
    #[serde(default)]
    pub segment_miles: Vec<f64>,
    pub sequence: u32,
    pub driver_id: Option<Uuid>,
    pub truck_id: Option<Uuid>,
    pub trailer_ids: Vec<Uuid>,
    pub status: TripStatus,
    pub stops: Vec<TripStop>,
    pub notes: Option<String>,
    /// Back-office annotation: mileage derivation, trip-chain links, equipment
    /// provenance, timestamp corrections, billing detail. Never serialized by any
    /// `/driver/api/v1` handler — the driver surface builds its own response
    /// structs and does not map this field.
    /// `tests/it/driver_internal_notes_test.rs` pins that.
    #[serde(default)]
    pub internal_notes: Option<String>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
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
    #[serde(default)]
    pub settlement_ref: Option<String>,
    #[serde(default)]
    pub pay_period_start: Option<String>,
    #[serde(default)]
    pub pay_period_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver_pay_snapshot: Option<crate::models::pay::DriverPay>,
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
    /// Dispatcher-only annotation. Never reaches the driver surface.
    pub internal_notes: Option<String>,
    pub previous_trip_id: Option<Uuid>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateTripRequest {
    pub load_id: Option<Uuid>,
    pub sequence: Option<u32>,
    pub stops: Option<Vec<TripStop>>,
    pub notes: Option<String>,
    pub blob_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TripListItem {
    pub id: Uuid,
    pub trip_number: String,
    pub load_id: Option<Uuid>,
    pub load_number: Option<String>,
    pub previous_trip_id: Option<Uuid>,
    pub deadhead_miles: Option<f64>,
    pub loaded_miles: Option<f64>,
    pub total_miles: Option<f64>,
    #[serde(default)]
    pub segment_miles: Vec<f64>,
    pub sequence: u32,
    pub driver_id: Option<Uuid>,
    pub truck_id: Option<Uuid>,
    pub trailer_ids: Vec<Uuid>,
    pub status: TripStatus,
    pub stops: Vec<TripStop>,
    pub notes: Option<String>,
    /// Dispatcher-only. This list type serves `/fleet` only.
    #[serde(default)]
    pub internal_notes: Option<String>,
    pub blob_ids: Vec<Uuid>,
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
    #[serde(default)]
    pub settlement_ref: Option<String>,
    #[serde(default)]
    pub pay_period_start: Option<String>,
    #[serde(default)]
    pub pay_period_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver_pay_snapshot: Option<crate::models::pay::DriverPay>,
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
            load_number: r.load_number,
            previous_trip_id: r.previous_trip_id,
            deadhead_miles: r.deadhead_miles,
            loaded_miles: r.loaded_miles,
            total_miles: r.total_miles,
            segment_miles: r.segment_miles,
            sequence: r.sequence,
            driver_id: r.driver_id,
            truck_id: r.truck_id,
            trailer_ids: r.trailer_ids,
            status: r.status,
            stops: r.stops,
            notes: r.notes,
            internal_notes: r.internal_notes,
            blob_ids: r.blob_ids,
            loaded_rate_per_mile: r.loaded_rate_per_mile,
            deadhead_rate_per_mile: r.deadhead_rate_per_mile,
            extra_stop_fee: r.extra_stop_fee,
            detention_rate_per_hour: r.detention_rate_per_hour,
            free_dwell_minutes: r.free_dwell_minutes,
            settlement_ref: r.settlement_ref,
            pay_period_start: r.pay_period_start,
            pay_period_end: r.pay_period_end,
            driver_pay_snapshot: r.driver_pay_snapshot,
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
        for s in ["origin", "fuel", "pickup", "delivery", "relay", "empty_move",
                  "maintenance", "terminal", "waypoint"] {
            let t: TripStopType = s.parse().unwrap();
            assert_eq!(t.as_str(), s);
        }
    }

    #[test]
    fn test_is_service_stop() {
        for t in [TripStopType::Pickup, TripStopType::Delivery,
                  TripStopType::Relay, TripStopType::EmptyMove] {
            assert!(t.is_service_stop(), "{t:?} is freight work");
        }
        // An empty move counts: it is a dispatched movement with its own BOL and
        // POD whose commodity happens to be nothing.
        for t in [TripStopType::Origin, TripStopType::Fuel, TripStopType::Maintenance,
                  TripStopType::Terminal, TripStopType::Waypoint] {
            assert!(!t.is_service_stop(), "{t:?} affects mileage but is not freight work");
        }
    }

    #[test]
    fn test_trip_status_roundtrip() {
        for s in ["planned", "assigned", "dispatched", "in_transit", "delivered", "completed", "cancelled", "tonu"] {
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
        assert!(TripStatus::Delivered.can_transition_to(&TripStatus::Completed));
        assert!(TripStatus::Planned.can_transition_to(&TripStatus::Cancelled));
        assert!(TripStatus::Assigned.can_transition_to(&TripStatus::Cancelled));
        assert!(TripStatus::Dispatched.can_transition_to(&TripStatus::Cancelled));
        assert!(!TripStatus::InTransit.can_transition_to(&TripStatus::Cancelled));
        assert!(!TripStatus::Delivered.can_transition_to(&TripStatus::Cancelled));
        assert!(!TripStatus::Planned.can_transition_to(&TripStatus::Delivered));
        assert!(!TripStatus::Delivered.can_transition_to(&TripStatus::Planned));
        assert!(!TripStatus::Completed.can_transition_to(&TripStatus::Planned));
        assert!(!TripStatus::Completed.can_transition_to(&TripStatus::Delivered));
        assert!(!TripStatus::Completed.can_transition_to(&TripStatus::Cancelled));
    }

    #[test]
    fn test_tonu_is_terminal_and_reachable_only_from_dispatched() {
        assert!(TripStatus::Dispatched.can_transition_to(&TripStatus::Tonu));
        for s in [TripStatus::Planned, TripStatus::Assigned, TripStatus::InTransit,
                  TripStatus::Delivered, TripStatus::Completed, TripStatus::Cancelled] {
            assert!(!s.can_transition_to(&TripStatus::Tonu), "{s:?} must not reach tonu");
        }
        for s in [TripStatus::Planned, TripStatus::Assigned, TripStatus::Dispatched,
                  TripStatus::InTransit, TripStatus::Delivered, TripStatus::Completed,
                  TripStatus::Cancelled] {
            assert!(!TripStatus::Tonu.can_transition_to(&s), "tonu is terminal, not -> {s:?}");
        }
        // A TONU'd trip delivered nothing.
        assert!(!TripStatus::Tonu.is_delivery_complete());
    }

    #[test]
    fn test_tonu_leg_is_a_dead_record_for_the_delivery_cascade() {
        use TripStatus::*;
        // A TONU'd leg is superseded by its replacement, exactly like a cancelled
        // one, so it is filtered out rather than held against the load. Counting
        // it would strand every relay load that TONUs a middle leg: Tonu has no
        // edge out and is never delivery-complete, so the load could never
        // cascade to Delivered and could never invoice.
        assert!(all_delivered(&[Delivered, Tonu]));
        // The real relay shape — deliver to the cross-dock, TONU at the dock,
        // re-dispatch and haul it out.
        assert!(all_delivered(&[Delivered, Tonu, Delivered]));
        assert!(all_delivered(&[Cancelled, Tonu, Completed]));
        // But a load whose only trip TONU'd has no live trip at all, so it is
        // not delivered — the non-empty guard, not the filter, carries this.
        assert!(!all_delivered(&[Tonu]));
        assert!(!all_delivered(&[Cancelled, Tonu]));
        // A live sibling still holds the load back.
        assert!(!all_delivered(&[Tonu, InTransit]));
    }

    // --- #395 ------------------------------------------------------------

    fn trip_with_status(status: TripStatus) -> TripRecord {
        let now = chrono::Utc::now();
        TripRecord {
            internal_notes: None,
            id: Uuid::new_v4(),
            trip_number: "T-2026-0001".into(),
            load_id: None,
            load_number: None,
            previous_trip_id: None,
            deadhead_miles: None,
            loaded_miles: None,
            total_miles: None,
            segment_miles: vec![],
            sequence: 0,
            driver_id: None,
            truck_id: None,
            trailer_ids: vec![],
            status,
            stops: vec![],
            notes: None,
            blob_ids: vec![],
            loaded_rate_per_mile: None,
            deadhead_rate_per_mile: None,
            extra_stop_fee: None,
            detention_rate_per_hour: None,
            free_dwell_minutes: None,
            settlement_ref: None,
            pay_period_start: None,
            pay_period_end: None,
            driver_pay_snapshot: None,
            embedding: None,
            owner_id: 0,
            created_at: now,
            updated_at: now,
        }
    }

    fn all_delivered(statuses: &[TripStatus]) -> bool {
        let trips: Vec<TripRecord> = statuses.iter().cloned().map(trip_with_status).collect();
        load_trips_all_delivered(&trips)
    }

    #[test]
    fn test_load_trips_all_delivered() {
        use TripStatus::*;

        assert!(all_delivered(&[Delivered]));
        assert!(all_delivered(&[Delivered, Delivered]));
        // A cancelled sibling is a dead record — it must not strand the load.
        assert!(all_delivered(&[Cancelled, Delivered]));
        // Completed is the normal end state a delivered trip moves on to; a relay
        // load's earlier leg is routinely completed before the last leg delivers.
        assert!(all_delivered(&[Completed, Delivered]));
        assert!(all_delivered(&[Cancelled, Completed, Delivered]));
        assert!(all_delivered(&[Completed]));

        // Anything still running holds the load.
        assert!(!all_delivered(&[Delivered, InTransit]));
        assert!(!all_delivered(&[Delivered, Planned]));
        assert!(!all_delivered(&[Delivered, Assigned]));
        assert!(!all_delivered(&[Delivered, Dispatched]));

        // No live trip means nothing delivered — not a vacuous true.
        assert!(!all_delivered(&[]));
        assert!(!all_delivered(&[Cancelled]));
        assert!(!all_delivered(&[Cancelled, Cancelled]));
    }

    #[test]
    fn test_is_delivery_complete() {
        assert!(TripStatus::Delivered.is_delivery_complete());
        assert!(TripStatus::Completed.is_delivery_complete());
        for s in [
            TripStatus::Planned,
            TripStatus::Assigned,
            TripStatus::Dispatched,
            TripStatus::InTransit,
            TripStatus::Cancelled,
        ] {
            assert!(!s.is_delivery_complete(), "{s:?} is not a delivery-complete state");
        }
    }

    #[test]
    fn test_embedding_text() {
        let now = chrono::Utc::now();
        let r = TripRecord {
            internal_notes: None,
            id: Uuid::new_v4(),
            trip_number: "T-2026-0001".into(),
            load_id: None,
            load_number: None,
            previous_trip_id: None,
            deadhead_miles: None,
            loaded_miles: None,
            total_miles: None,
            segment_miles: vec![],
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
                    address: None,
                    load_stop_index: None,
                    scheduled_arrive: None,
                    scheduled_arrive_end: None,
                    actual_arrive: None,
                    actual_depart: None,
                    expected_dwell_minutes: None,
                    detention_free_minutes: None,
                    detention_grace_minutes: None,
                    notes: None,
                    timezone: None,
                    actual_arrive_utc: None,
                    actual_depart_utc: None,
                },
            ],
            notes: Some("urgent".into()),
            blob_ids: vec![],
            loaded_rate_per_mile: None,
            deadhead_rate_per_mile: None,
            extra_stop_fee: None,
            detention_rate_per_hour: None,
            free_dwell_minutes: None,
            settlement_ref: None,
            pay_period_start: None,
            pay_period_end: None,
            driver_pay_snapshot: None,
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
            internal_notes: None,
            id: Uuid::new_v4(),
            trip_number: "T-2026-0001".into(),
            load_id: None,
            load_number: None,
            previous_trip_id: None,
            deadhead_miles: None,
            loaded_miles: None,
            total_miles: None,
            segment_miles: vec![],
            sequence: 0,
            driver_id: None,
            truck_id: None,
            trailer_ids: vec![],
            status: TripStatus::Planned,
            stops: vec![],
            notes: None,
            blob_ids: vec![],
            loaded_rate_per_mile: None,
            deadhead_rate_per_mile: None,
            extra_stop_fee: None,
            detention_rate_per_hour: None,
            free_dwell_minutes: None,
            settlement_ref: None,
            pay_period_start: None,
            pay_period_end: None,
            driver_pay_snapshot: None,
            embedding: Some(vec![0.1, 0.2]),
            owner_id: 0,
            created_at: now,
            updated_at: now,
        };
        let json = serde_json::to_value(&r).unwrap();
        assert!(json.get("embedding").is_none());
    }
}
