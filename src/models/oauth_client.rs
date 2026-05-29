// src/models/oauth_client.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A dynamically-registered public OAuth client (RFC 7591). No secret (PKCE).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OAuthClient {
    pub id: Uuid,
    pub client_name: Option<String>,
    pub redirect_uris: Vec<String>,
    pub created_at: DateTime<Utc>,
}
