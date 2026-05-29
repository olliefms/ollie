// src/models/authorization_code.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A one-time authorization code bound to a PKCE challenge and a subject.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuthorizationCode {
    pub code_hash: String,
    pub client_id: Uuid,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub subject_type: String,
    pub subject_id: Uuid,
    pub resource: String,
    pub scope: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
}
