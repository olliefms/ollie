// src/models/fleet_user_credentials.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetUserCredentials {
    pub fleet_user_id: Uuid,
    pub password_hash: String,
    pub token_version: i64,
    pub failed_attempts: i32,
    pub locked_until: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}
