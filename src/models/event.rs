use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// Classify an event's display severity from its type.
/// Exception wins over system when both could apply.
pub fn classify_severity(event_type: &str) -> &'static str {
    match event_type {
        "stop.late" | "processing_failed" => "exception",
        "processing_started" | "processing_completed" | "driver.equipment_changed"
        | "driver.trailer_changed" => "system",
        _ => "normal",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EventRecord {
    pub id: Uuid,
    pub entity_type: String,
    pub entity_id: Uuid,
    pub event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    pub occurred_at: String,
    #[serde(skip)]
    #[schema(skip)]
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EventResponse {
    pub id: Uuid,
    pub entity_type: String,
    pub entity_id: Uuid,
    pub event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    pub occurred_at: String,
}

impl From<EventRecord> for EventResponse {
    fn from(r: EventRecord) -> Self {
        Self {
            id: r.id,
            entity_type: r.entity_type,
            entity_id: r.entity_id,
            event_type: r.event_type,
            payload: r.payload.as_deref().and_then(|s| serde_json::from_str(s).ok()),
            actor: r.actor,
            occurred_at: r.occurred_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EventListResponse {
    pub returned: usize,
    pub items: Vec<EventResponse>,
}

#[cfg(test)]
mod tests {
    use super::classify_severity;

    #[test]
    fn classifies_severity() {
        assert_eq!(classify_severity("stop.late"), "exception");
        assert_eq!(classify_severity("processing_failed"), "exception");
        assert_eq!(classify_severity("processing_started"), "system");
        assert_eq!(classify_severity("processing_completed"), "system");
        assert_eq!(classify_severity("driver.equipment_changed"), "system");
        assert_eq!(classify_severity("driver.trailer_changed"), "system");
        assert_eq!(classify_severity("trip.dispatched"), "normal");
        assert_eq!(classify_severity("stop.arrived"), "normal");
        assert_eq!(classify_severity("check_call"), "normal");
        assert_eq!(classify_severity("anything_else"), "normal");
    }
}
