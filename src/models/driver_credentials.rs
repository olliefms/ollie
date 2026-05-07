use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverCredentials {
    pub driver_id: Uuid,
    pub pin_hash: Option<String>,
    pub token_version: i64,
    pub failed_pin_attempts: i64,
    pub locked_until: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverPasskeyCredential {
    pub credential_id: String,
    pub driver_id: Uuid,
    pub public_key: String,
    pub counter: i64,
    pub transports: String,
    pub created_at: DateTime<Utc>,
}
