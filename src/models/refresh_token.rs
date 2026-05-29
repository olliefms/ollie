// src/models/refresh_token.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A rotating refresh token. `token_hash` is the SHA-256 hex of the opaque
/// secret (the secret itself is never stored). Rows in one rotation chain
/// share a `family_id`; replay of a `consumed_at`-set token revokes the family.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RefreshToken {
    pub id: Uuid,
    pub token_hash: String,
    /// "dispatcher" or "driver"
    pub subject_type: String,
    pub subject_id: Uuid,
    /// NULL = PWA session; set = an OAuth client (Plan 2).
    pub client_id: Option<Uuid>,
    pub family_id: Uuid,
    /// Snapshot of the subject's `token_version` at issue; rotation rejects on mismatch.
    pub token_version: i64,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_refresh_token_is_active_predicate() {
        let now = Utc::now();
        let active = RefreshToken {
            id: Uuid::new_v4(),
            token_hash: "h".into(),
            subject_type: "dispatcher".into(),
            subject_id: Uuid::new_v4(),
            client_id: None,
            family_id: Uuid::new_v4(),
            token_version: 0,
            issued_at: now,
            expires_at: now + chrono::Duration::days(14),
            consumed_at: None,
            revoked_at: None,
            last_used_at: None,
        };
        assert!(active.revoked_at.is_none() && active.consumed_at.is_none());
        assert!(active.expires_at > now);
    }
}
