// src/api/blob.rs
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, ToSchema)]
pub struct BlobQueryRequest {
    /// The question to ask about the document (1–4096 characters)
    pub prompt: String,
    /// Ollama model to use (defaults to OLLAMA_SUMMARY_MODEL)
    pub model: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BlobQueryResponse {
    pub id: Uuid,
    pub prompt: String,
    pub answer: String,
    pub model: String,
    pub processing_time_ms: u64,
}
