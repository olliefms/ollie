// src/models/dispatcher_api_key.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatcherApiKey {
    pub id: Uuid,
    pub dispatcher_id: Uuid,
    pub label: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dispatcher_api_key_clone() {
        let now = Utc::now();
        let key = DispatcherApiKey {
            id: Uuid::new_v4(),
            dispatcher_id: Uuid::new_v4(),
            label: "test".into(),
            key_hash: "abc".into(),
            key_prefix: "olld_a1b2c3".into(),
            created_at: now,
            expires_at: now + chrono::Duration::days(365),
            revoked_at: None,
            last_used_at: None,
        };
        let clone = key.clone();
        assert_eq!(clone.id, key.id);
        assert_eq!(clone.label, "test");
    }
}
