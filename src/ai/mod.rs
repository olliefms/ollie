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
            .map_err(|e| AppError::Internal(format!("ollama embed status: {e}")))?
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
                "ollama generate status: {status} — {body}"
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
