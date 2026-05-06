// src/models/blob.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BlobStatus {
    Pending,
    Processing,
    Ready,
    Failed,
}

impl BlobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Processing => "processing",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

impl std::str::FromStr for BlobStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "processing" => Ok(Self::Processing),
            "ready" => Ok(Self::Ready),
            "failed" => Ok(Self::Failed),
            other => Err(format!("unknown status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobRecord {
    pub id: Uuid,
    pub owner_id: i64,
    pub checksum: String,
    pub name: String,
    pub mime_type: String,
    pub size: i64,
    pub status: BlobStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub tags: Vec<String>,
    #[serde(skip)]
    pub embedding: Option<Vec<f32>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateBlobRequest {
    pub name: Option<String>,
    pub tags: Option<Vec<String>>,
}

/// Returned by GET /api/v1/blobs — no embedding, optional score
#[derive(Debug, Clone, Serialize)]
pub struct BlobListItem {
    pub id: Uuid,
    pub owner_id: i64,
    pub name: String,
    pub mime_type: String,
    pub size: i64,
    pub status: BlobStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

impl From<BlobRecord> for BlobListItem {
    fn from(r: BlobRecord) -> Self {
        Self {
            id: r.id, owner_id: r.owner_id, name: r.name,
            mime_type: r.mime_type, size: r.size, status: r.status,
            summary: r.summary, tags: r.tags, created_at: r.created_at,
            score: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct BlobListResponse {
    pub total: usize,
    pub items: Vec<BlobListItem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_roundtrip() {
        for s in ["pending", "processing", "ready", "failed"] {
            let status: BlobStatus = s.parse().unwrap();
            assert_eq!(status.as_str(), s);
        }
    }

    #[test]
    fn test_blob_record_embedding_skipped_in_json() {
        let record = BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum: "abc".into(),
            name: "file.txt".into(), mime_type: "text/plain".into(), size: 100,
            status: BlobStatus::Ready, error: None, summary: Some("a summary".into()),
            tags: vec!["a".into()],
            embedding: Some(vec![0.1, 0.2, 0.3]),
            created_at: Utc::now(), updated_at: Utc::now(),
        };
        let json = serde_json::to_value(&record).unwrap();
        assert!(json.get("embedding").is_none(), "embedding must not appear in JSON output");
        assert!(json.get("error").is_none());
    }
}
