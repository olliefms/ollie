// src/ai/mod.rs
pub mod embed;
pub mod expense_fields;
pub mod extract;
pub mod ocr;
pub mod summarize;

use crate::error::AppError;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct OllamaClient {
    pub base_url: String,
    pub embed_model: String,
    pub summary_model: String,
    pub vision_model: String,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    prompt: &'a str,
}

#[derive(Deserialize)]
struct EmbedResponse {
    embedding: Vec<f32>,
}

#[derive(Serialize)]
struct GenerateRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    images: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct GenerateResponse {
    response: String,
}

/// Prefixes for the two Ollama errors that mean "the service answered, and it
/// refused *these bytes*" — a non-2xx with the model up. They are consts
/// because `classify_ollama_error` matches on them: constructor and classifier
/// share one spelling so they cannot drift apart.
///
/// Everything else the client can return — a transport error, a parse error —
/// means we never got an answer, which says nothing about the document.
pub const EMBED_STATUS_PREFIX: &str = "ollama embed status:";
pub const GENERATE_STATUS_PREFIX: &str = "ollama generate status:";

/// Did this failure implicate the document, or the dependency?
///
/// Only a non-2xx from a reachable Ollama is document-scoped — that is the
/// vision-model context overflow case (AGENTS.md), where the same bytes fail
/// the same way every time and retrying forever is pure waste. Connection
/// refused, timeouts and malformed responses are the dependency's problem, and
/// must not spend a blob's retry budget: three restarts during one outage would
/// otherwise write off every document that happened to be queued (#406).
///
/// Anything unrecognised defaults to `Dependency` — the safe direction, since
/// it costs a retry rather than a document.
pub fn classify_ollama_error(err: &AppError) -> crate::models::BlobFailureKind {
    use crate::models::BlobFailureKind;
    match err {
        AppError::Internal(msg)
            if msg.starts_with(EMBED_STATUS_PREFIX) || msg.starts_with(GENERATE_STATUS_PREFIX) =>
        {
            BlobFailureKind::Document
        }
        _ => BlobFailureKind::Dependency,
    }
}

impl OllamaClient {
    pub fn new(base_url: &str, embed_model: &str, summary_model: &str, vision_model: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            embed_model: embed_model.to_string(),
            summary_model: summary_model.to_string(),
            vision_model: vision_model.to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, AppError> {
        let resp: EmbedResponse = self.client
            .post(format!("{}/api/embeddings", self.base_url))
            .json(&EmbedRequest { model: &self.embed_model, prompt: text })
            .send().await
            .map_err(|e| AppError::Internal(format!("ollama embed: {e}")))?
            .error_for_status()
            .map_err(|e| AppError::Internal(format!("{EMBED_STATUS_PREFIX} {e}")))?
            .json().await
            .map_err(|e| AppError::Internal(format!("ollama embed parse: {e}")))?;
        Ok(resp.embedding)
    }

    pub async fn generate(&self, model: &str, prompt: &str, image_b64: Option<String>) -> Result<String, AppError> {
        let resp = self.client
            .post(format!("{}/api/generate", self.base_url))
            .json(&GenerateRequest {
                model, prompt, stream: false,
                images: image_b64.map(|b| vec![b]),
            })
            .send().await
            .map_err(|e| AppError::Internal(format!("ollama generate: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Internal(format!(
                "{GENERATE_STATUS_PREFIX} {status} — {body}"
            )));
        }
        let parsed: GenerateResponse = resp.json().await
            .map_err(|e| AppError::Internal(format!("ollama generate parse: {e}")))?;
        Ok(parsed.response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The split that decides whether a blob spends its retry budget (#406).
    /// These match the exact strings `embed`/`generate` construct above — the
    /// shared consts are what keep the two sides from drifting.
    #[test]
    fn test_classify_ollama_error() {
        use crate::models::BlobFailureKind;

        // Reachable model, non-2xx: it saw these bytes and refused them.
        for msg in [
            format!("{EMBED_STATUS_PREFIX} HTTP status client error (413 Payload Too Large)"),
            format!("{GENERATE_STATUS_PREFIX} 500 Internal Server Error — context overflow"),
        ] {
            assert_eq!(
                classify_ollama_error(&AppError::Internal(msg.clone())),
                BlobFailureKind::Document,
                "a non-2xx from a reachable model is document-scoped: {msg}"
            );
        }

        // Never got an answer — says nothing about the document.
        for msg in [
            "ollama embed: error sending request for url (http://127.0.0.1:1)",
            "ollama generate: connection refused",
            "ollama embed parse: expected value at line 1",
            "ollama generate parse: EOF while parsing",
        ] {
            assert_eq!(
                classify_ollama_error(&AppError::Internal(msg.into())),
                BlobFailureKind::Dependency,
                "an unanswered call must not spend the budget: {msg}"
            );
        }

        // Anything unrecognised defaults to the safe direction.
        assert_eq!(
            classify_ollama_error(&AppError::NotFound),
            BlobFailureKind::Dependency,
        );
    }

    #[test]
    fn test_client_constructs() {
        let c = OllamaClient::new("http://localhost:11434", "nomic-embed-text", "llama3.2", "moondream");
        assert_eq!(c.embed_model, "nomic-embed-text");
    }

    #[test]
    fn test_base_url_strips_trailing_slash() {
        let c = OllamaClient::new("http://localhost:11434/", "a", "b", "c");
        assert_eq!(c.base_url, "http://localhost:11434");
    }
}
