// src/models/fleet_user.rs
use crate::models::permission::Role;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FleetUserStatus {
    Active,
    Inactive,
}

impl FleetUserStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
        }
    }
}

impl std::str::FromStr for FleetUserStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(Self::Active),
            "inactive" => Ok(Self::Inactive),
            other => Err(format!("unknown fleet_user status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FleetUserRecord {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub status: FleetUserStatus,
    #[serde(default)]
    pub role: Role,
    #[serde(default)]
    pub extra_scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fleet_user_status_roundtrip() {
        for s in ["active", "inactive"] {
            let st: FleetUserStatus = s.parse().unwrap();
            assert_eq!(st.as_str(), s);
        }
    }

    #[test]
    fn test_fleet_user_status_unknown() {
        assert!("pending".parse::<FleetUserStatus>().is_err());
    }
}
