// src/models/expense.rs
//
// Expense tracking (see docs/superpowers/specs/2026-07-21-expense-tracking-design.md).
// Money effects are DERIVED, never stored: the review decision (approved_amount vs
// amount) combines with payment_method to yield a reimbursement (personal) or a
// deduction (company). suggested_* fields are AI-staged scaffolding cleared at review.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::models::EquipmentType;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExpenseCategory {
    Fuel,
    Tolls,
    Scales,
    Lumper,
    Parking,
    Repair,
    Supplies,
    Permit,
    Other,
}

impl ExpenseCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fuel => "fuel",
            Self::Tolls => "tolls",
            Self::Scales => "scales",
            Self::Lumper => "lumper",
            Self::Parking => "parking",
            Self::Repair => "repair",
            Self::Supplies => "supplies",
            Self::Permit => "permit",
            Self::Other => "other",
        }
    }
}

impl std::str::FromStr for ExpenseCategory {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "fuel" => Ok(Self::Fuel),
            "tolls" => Ok(Self::Tolls),
            "scales" => Ok(Self::Scales),
            "lumper" => Ok(Self::Lumper),
            "parking" => Ok(Self::Parking),
            "repair" => Ok(Self::Repair),
            "supplies" => Ok(Self::Supplies),
            "permit" => Ok(Self::Permit),
            "other" => Ok(Self::Other),
            other => Err(format!("unknown expense category: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExpenseStatus {
    Submitted,
    Reviewed,
    Settled,
}

impl ExpenseStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Reviewed => "reviewed",
            Self::Settled => "settled",
        }
    }
}

impl std::str::FromStr for ExpenseStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "submitted" => Ok(Self::Submitted),
            "reviewed" => Ok(Self::Reviewed),
            "settled" => Ok(Self::Settled),
            other => Err(format!("unknown expense status: {other}")),
        }
    }
}

/// `company` covers ANY company funds: fleet card, check, comcheck, ACH.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PaymentMethod {
    Company,
    Personal,
}

impl PaymentMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Company => "company",
            Self::Personal => "personal",
        }
    }
}

impl std::str::FromStr for PaymentMethod {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "company" => Ok(Self::Company),
            "personal" => Ok(Self::Personal),
            other => Err(format!("unknown payment method: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ExpenseRecord {
    pub id: Uuid,
    pub status: ExpenseStatus,
    pub category: ExpenseCategory,
    pub driver_id: Option<Uuid>,
    pub trip_id: Option<Uuid>,
    pub equipment_type: Option<EquipmentType>,
    pub equipment_id: Option<Uuid>,
    pub maintenance_id: Option<Uuid>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
    /// Ownership marker: `driver:<uuid>` or `fleet_user:<uuid>`.
    pub submitted_by: String,
    /// ISO date `YYYY-MM-DD`; set at review.
    pub expense_date: Option<String>,
    pub vendor: Option<String>,
    /// Receipt total in USD; set at review.
    pub amount: Option<f64>,
    /// 0 <= approved_amount <= amount; set at review.
    pub approved_amount: Option<f64>,
    pub payment_method: Option<PaymentMethod>,
    pub suggested_amount: Option<f64>,
    pub suggested_date: Option<String>,
    pub suggested_vendor: Option<String>,
    pub suggested_card_last4: Option<String>,
    /// Fleet user UUID string of the reviewer.
    pub reviewed_by: Option<String>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub review_note: Option<String>,
    /// Set by the future settlements feature; locks the record permanently.
    pub settlement_id: Option<Uuid>,
    #[serde(skip)]
    #[schema(skip)]
    pub embedding: Option<Vec<f32>>,
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ExpenseRecord {
    /// Amount owed TO the driver on settlement (personal funds, approved portion).
    pub fn reimbursement(&self) -> Option<f64> {
        match (self.status, self.payment_method, self.approved_amount) {
            (ExpenseStatus::Submitted, _, _) => None,
            (_, Some(PaymentMethod::Personal), Some(a)) if a > 0.0 => Some(a),
            _ => None,
        }
    }

    /// Amount deducted FROM the driver on settlement (company funds, denied portion).
    pub fn deduction(&self) -> Option<f64> {
        match (self.status, self.payment_method, self.amount, self.approved_amount) {
            (ExpenseStatus::Submitted, _, _, _) => None,
            (_, Some(PaymentMethod::Company), Some(t), Some(a)) if t - a > 0.0 => Some(t - a),
            _ => None,
        }
    }

    /// "approved" | "partial" | "rejected", None until reviewed.
    pub fn disposition(&self) -> Option<&'static str> {
        if matches!(self.status, ExpenseStatus::Submitted) {
            return None;
        }
        match (self.amount, self.approved_amount) {
            (Some(t), Some(a)) if a <= 0.0 && t > 0.0 => Some("rejected"),
            (Some(t), Some(a)) if a < t => Some("partial"),
            (Some(_), Some(_)) => Some("approved"),
            _ => None,
        }
    }

    pub fn is_locked(&self) -> bool {
        self.settlement_id.is_some()
    }

    pub fn embedding_text(&self) -> String {
        format!(
            "expense {} {} {} {}",
            self.category.as_str(),
            self.vendor.as_deref().unwrap_or(""),
            self.expense_date.as_deref().unwrap_or(""),
            self.review_note.as_deref().unwrap_or("")
        )
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ExpenseResponse {
    pub id: Uuid,
    pub status: ExpenseStatus,
    pub category: ExpenseCategory,
    pub driver_id: Option<Uuid>,
    pub trip_id: Option<Uuid>,
    pub equipment_type: Option<EquipmentType>,
    pub equipment_id: Option<Uuid>,
    pub maintenance_id: Option<Uuid>,
    pub blob_ids: Vec<Uuid>,
    pub submitted_by: String,
    pub expense_date: Option<String>,
    pub vendor: Option<String>,
    pub amount: Option<f64>,
    pub approved_amount: Option<f64>,
    pub payment_method: Option<PaymentMethod>,
    pub suggested_amount: Option<f64>,
    pub suggested_date: Option<String>,
    pub suggested_vendor: Option<String>,
    pub suggested_card_last4: Option<String>,
    pub reviewed_by: Option<String>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub review_note: Option<String>,
    pub settlement_id: Option<Uuid>,
    /// Derived: approved portion when payment_method=personal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reimbursement: Option<f64>,
    /// Derived: denied portion when payment_method=company.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deduction: Option<f64>,
    /// Derived: "approved" | "partial" | "rejected".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disposition: Option<&'static str>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ExpenseRecord> for ExpenseResponse {
    fn from(r: ExpenseRecord) -> Self {
        let reimbursement = r.reimbursement();
        let deduction = r.deduction();
        let disposition = r.disposition();
        Self {
            id: r.id,
            status: r.status,
            category: r.category,
            driver_id: r.driver_id,
            trip_id: r.trip_id,
            equipment_type: r.equipment_type,
            equipment_id: r.equipment_id,
            maintenance_id: r.maintenance_id,
            blob_ids: r.blob_ids,
            submitted_by: r.submitted_by,
            expense_date: r.expense_date,
            vendor: r.vendor,
            amount: r.amount,
            approved_amount: r.approved_amount,
            payment_method: r.payment_method,
            suggested_amount: r.suggested_amount,
            suggested_date: r.suggested_date,
            suggested_vendor: r.suggested_vendor,
            suggested_card_last4: r.suggested_card_last4,
            reviewed_by: r.reviewed_by,
            reviewed_at: r.reviewed_at,
            review_note: r.review_note,
            settlement_id: r.settlement_id,
            reimbursement,
            deduction,
            disposition,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExpenseListResponse {
    pub returned: usize,
    pub total: usize,
    pub items: Vec<ExpenseResponse>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ExpenseRecord {
        let now = Utc::now();
        ExpenseRecord {
            id: Uuid::new_v4(),
            status: ExpenseStatus::Submitted,
            category: ExpenseCategory::Fuel,
            driver_id: Some(Uuid::new_v4()),
            trip_id: None,
            equipment_type: None,
            equipment_id: None,
            maintenance_id: None,
            blob_ids: vec![],
            submitted_by: format!("driver:{}", Uuid::new_v4()),
            expense_date: None,
            vendor: None,
            amount: None,
            approved_amount: None,
            payment_method: None,
            suggested_amount: None,
            suggested_date: None,
            suggested_vendor: None,
            suggested_card_last4: None,
            reviewed_by: None,
            reviewed_at: None,
            review_note: None,
            settlement_id: None,
            embedding: None,
            owner_id: 0,
            created_at: now,
            updated_at: now,
        }
    }

    fn reviewed(amount: f64, approved: f64, method: PaymentMethod) -> ExpenseRecord {
        let mut r = base();
        r.status = ExpenseStatus::Reviewed;
        r.amount = Some(amount);
        r.approved_amount = Some(approved);
        r.payment_method = Some(method);
        r
    }

    #[test]
    fn test_enum_roundtrips() {
        for s in ["fuel","tolls","scales","lumper","parking","repair","supplies","permit","other"] {
            let c: ExpenseCategory = s.parse().unwrap();
            assert_eq!(c.as_str(), s);
        }
        for s in ["submitted","reviewed","settled"] {
            let c: ExpenseStatus = s.parse().unwrap();
            assert_eq!(c.as_str(), s);
        }
        for s in ["company","personal"] {
            let c: PaymentMethod = s.parse().unwrap();
            assert_eq!(c.as_str(), s);
        }
        assert!("cash".parse::<PaymentMethod>().is_err());
    }

    #[test]
    fn test_no_money_effects_until_reviewed() {
        let r = base();
        assert_eq!(r.reimbursement(), None);
        assert_eq!(r.deduction(), None);
        assert_eq!(r.disposition(), None);
    }

    #[test]
    fn test_personal_full_approval_reimburses() {
        let r = reviewed(100.0, 100.0, PaymentMethod::Personal);
        assert_eq!(r.reimbursement(), Some(100.0));
        assert_eq!(r.deduction(), None);
        assert_eq!(r.disposition(), Some("approved"));
    }

    #[test]
    fn test_personal_partial_reimburses_approved_portion() {
        let r = reviewed(100.0, 80.0, PaymentMethod::Personal);
        assert_eq!(r.reimbursement(), Some(80.0));
        assert_eq!(r.deduction(), None);
        assert_eq!(r.disposition(), Some("partial"));
    }

    #[test]
    fn test_personal_rejection_no_effect() {
        let r = reviewed(100.0, 0.0, PaymentMethod::Personal);
        assert_eq!(r.reimbursement(), None);
        assert_eq!(r.deduction(), None);
        assert_eq!(r.disposition(), Some("rejected"));
    }

    #[test]
    fn test_company_full_approval_no_effect() {
        let r = reviewed(100.0, 100.0, PaymentMethod::Company);
        assert_eq!(r.reimbursement(), None);
        assert_eq!(r.deduction(), None);
        assert_eq!(r.disposition(), Some("approved"));
    }

    #[test]
    fn test_company_partial_deducts_denied_portion() {
        let r = reviewed(100.0, 80.0, PaymentMethod::Company);
        assert_eq!(r.reimbursement(), None);
        assert!((r.deduction().unwrap() - 20.0).abs() < 1e-9);
        assert_eq!(r.disposition(), Some("partial"));
    }

    #[test]
    fn test_company_rejection_deducts_everything() {
        let r = reviewed(100.0, 0.0, PaymentMethod::Company);
        assert_eq!(r.deduction(), Some(100.0));
        assert_eq!(r.disposition(), Some("rejected"));
    }

    #[test]
    fn test_settlement_lock() {
        let mut r = reviewed(50.0, 50.0, PaymentMethod::Personal);
        assert!(!r.is_locked());
        r.settlement_id = Some(Uuid::new_v4());
        assert!(r.is_locked());
    }

    #[test]
    fn test_embedding_skipped_and_derived_fields_serialize() {
        let r = reviewed(100.0, 80.0, PaymentMethod::Company);
        let resp: ExpenseResponse = r.into();
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("embedding").is_none());
        assert_eq!(json["disposition"], "partial");
        assert!((json["deduction"].as_f64().unwrap() - 20.0).abs() < 1e-9);
        assert_eq!(json["payment_method"], "company");
    }
}
