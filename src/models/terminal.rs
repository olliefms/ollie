// src/models/terminal.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// A terminal (yard/HQ). Anchors pay-period timezone and the mandatory rate floor.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TerminalRecord {
    pub id: Uuid,
    pub name: String,
    /// Freeform address string (no dedicated Address struct exists in this codebase;
    /// facilities use a plain String — we match that and allow null).
    pub address: Option<String>,
    /// IANA timezone name, e.g. "America/New_York".
    pub timezone: String,
    /// Exactly one terminal is the default (used as the resolution floor / seed).
    pub is_default: bool,
    // --- mandatory rate floor: always concrete ---
    pub loaded_rate_per_mile: f64,
    pub deadhead_rate_per_mile: f64,
    pub extra_stop_fee: f64,
    pub detention_rate_per_hour: f64,
    pub free_dwell_minutes: u32,
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTerminalRequest {
    pub name: String,
    #[serde(default)]
    pub address: Option<String>,
    pub timezone: String,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub loaded_rate_per_mile: f64,
    #[serde(default)]
    pub deadhead_rate_per_mile: f64,
    #[serde(default)]
    pub extra_stop_fee: f64,
    #[serde(default)]
    pub detention_rate_per_hour: f64,
    /// Defaults to 120 if omitted.
    #[serde(default = "default_free_dwell")]
    pub free_dwell_minutes: u32,
}

fn default_free_dwell() -> u32 {
    120
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateTerminalRequest {
    pub name: Option<String>,
    pub address: Option<String>,
    pub timezone: Option<String>,
    pub is_default: Option<bool>,
    pub loaded_rate_per_mile: Option<f64>,
    pub deadhead_rate_per_mile: Option<f64>,
    pub extra_stop_fee: Option<f64>,
    pub detention_rate_per_hour: Option<f64>,
    pub free_dwell_minutes: Option<u32>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TerminalListItem {
    pub id: Uuid,
    pub name: String,
    pub address: Option<String>,
    pub timezone: String,
    pub is_default: bool,
    pub loaded_rate_per_mile: f64,
    pub deadhead_rate_per_mile: f64,
    pub extra_stop_fee: f64,
    pub detention_rate_per_hour: f64,
    pub free_dwell_minutes: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<TerminalRecord> for TerminalListItem {
    fn from(r: TerminalRecord) -> Self {
        Self {
            id: r.id,
            name: r.name,
            address: r.address,
            timezone: r.timezone,
            is_default: r.is_default,
            loaded_rate_per_mile: r.loaded_rate_per_mile,
            deadhead_rate_per_mile: r.deadhead_rate_per_mile,
            extra_stop_fee: r.extra_stop_fee,
            detention_rate_per_hour: r.detention_rate_per_hour,
            free_dwell_minutes: r.free_dwell_minutes,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}
