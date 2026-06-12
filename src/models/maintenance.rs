// src/models/maintenance.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EquipmentType {
    Truck,
    Trailer,
}

impl EquipmentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Truck => "truck",
            Self::Trailer => "trailer",
        }
    }
}

impl std::str::FromStr for EquipmentType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "truck" => Ok(Self::Truck),
            "trailer" => Ok(Self::Trailer),
            other => Err(format!("unknown equipment type: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceCategory {
    PreventiveMaintenance,
    Repair,
    Tire,
    Inspection,
    OilChange,
    Brakes,
    Other,
}

impl MaintenanceCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PreventiveMaintenance => "preventive_maintenance",
            Self::Repair => "repair",
            Self::Tire => "tire",
            Self::Inspection => "inspection",
            Self::OilChange => "oil_change",
            Self::Brakes => "brakes",
            Self::Other => "other",
        }
    }
}

impl std::str::FromStr for MaintenanceCategory {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "preventive_maintenance" => Ok(Self::PreventiveMaintenance),
            "repair" => Ok(Self::Repair),
            "tire" => Ok(Self::Tire),
            "inspection" => Ok(Self::Inspection),
            "oil_change" => Ok(Self::OilChange),
            "brakes" => Ok(Self::Brakes),
            "other" => Ok(Self::Other),
            other => Err(format!("unknown maintenance category: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MaintenanceRecord {
    pub id: Uuid,
    pub equipment_type: EquipmentType,
    pub equipment_id: Uuid,
    /// ISO date string `YYYY-MM-DD` for when the work was performed.
    pub service_date: String,
    pub category: MaintenanceCategory,
    pub description: String,
    pub cost: Option<f64>,
    pub odometer: Option<i64>,
    pub vendor: Option<String>,
    pub invoice_ref: Option<String>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
    #[serde(skip)]
    #[schema(skip)]
    pub embedding: Option<Vec<f32>>,
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl MaintenanceRecord {
    /// Base embedding text. The write handler prepends the parent equipment's
    /// unit number so entries are searchable by unit (e.g. "brake job TR-100").
    pub fn embedding_text(&self) -> String {
        format!(
            "{} {} {}",
            self.category.as_str(),
            self.description,
            self.vendor.as_deref().unwrap_or("")
        )
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MaintenanceListItem {
    pub id: Uuid,
    pub equipment_type: EquipmentType,
    pub equipment_id: Uuid,
    pub service_date: String,
    pub category: MaintenanceCategory,
    pub description: String,
    pub cost: Option<f64>,
    pub odometer: Option<i64>,
    pub vendor: Option<String>,
    pub invoice_ref: Option<String>,
    pub blob_ids: Vec<Uuid>,
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

impl From<MaintenanceRecord> for MaintenanceListItem {
    fn from(r: MaintenanceRecord) -> Self {
        Self {
            id: r.id,
            equipment_type: r.equipment_type,
            equipment_id: r.equipment_id,
            service_date: r.service_date,
            category: r.category,
            description: r.description,
            cost: r.cost,
            odometer: r.odometer,
            vendor: r.vendor,
            invoice_ref: r.invoice_ref,
            blob_ids: r.blob_ids,
            owner_id: r.owner_id,
            created_at: r.created_at,
            score: None,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MaintenanceListResponse {
    pub returned: usize,
    pub items: Vec<MaintenanceListItem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_equipment_type_roundtrip() {
        for s in ["truck", "trailer"] {
            let t: EquipmentType = s.parse().unwrap();
            assert_eq!(t.as_str(), s);
        }
    }

    #[test]
    fn test_equipment_type_unknown() {
        assert!("bus".parse::<EquipmentType>().is_err());
    }

    #[test]
    fn test_maintenance_category_roundtrip() {
        for s in [
            "preventive_maintenance", "repair", "tire", "inspection",
            "oil_change", "brakes", "other",
        ] {
            let c: MaintenanceCategory = s.parse().unwrap();
            assert_eq!(c.as_str(), s);
        }
    }

    #[test]
    fn test_maintenance_category_unknown() {
        assert!("transmission".parse::<MaintenanceCategory>().is_err());
    }

    #[test]
    fn test_record_embedding_skipped_in_json() {
        let now = Utc::now();
        let r = MaintenanceRecord {
            id: Uuid::new_v4(),
            equipment_type: EquipmentType::Truck,
            equipment_id: Uuid::new_v4(),
            service_date: "2026-06-01".into(),
            category: MaintenanceCategory::Repair,
            description: "replaced alternator".into(),
            cost: Some(412.50),
            odometer: Some(184000),
            vendor: Some("Acme Diesel".into()),
            invoice_ref: Some("INV-9931".into()),
            blob_ids: vec![],
            embedding: Some(vec![0.1]),
            owner_id: 0,
            created_at: now,
            updated_at: now,
        };
        let json = serde_json::to_value(&r).unwrap();
        assert!(json.get("embedding").is_none());
        assert_eq!(json["category"], "repair");
        assert_eq!(json["equipment_type"], "truck");
    }
}
