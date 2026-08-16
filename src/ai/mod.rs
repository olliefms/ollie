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

/// Tag for "the model answered with a non-2xx". The numeric status follows
/// immediately, so `classify_ollama_error` can read it back — constructor and
/// classifier share this const, and the code is carried as data rather than
/// inferred from prose.
pub const OLLAMA_STATUS_PREFIX: &str = "ollama status ";

/// Upstream bodies land in `BlobRecord::error` and in the `processing_failed`
/// event, and dependency failures are retried on every restart — so a proxy
/// that answers 5xx with a full HTML page would otherwise grow both by that
/// page per blob per restart. Ollama's own errors are one short line.
const MAX_ERROR_BODY: usize = 500;

/// Build the error for a non-2xx from Ollama, keeping the response body.
///
/// Deliberately not `error_for_status()`: that consumes the response and
/// discards the body, which is where Ollama explains itself (AGENTS.md).
fn status_err(op: &str, status: reqwest::StatusCode, body: &str) -> AppError {
    let body = summarize::truncate_at_char_boundary(body.trim(), MAX_ERROR_BODY);
    AppError::Internal(format!(
        "{OLLAMA_STATUS_PREFIX}{}: {op} — {body}",
        status.as_u16()
    ))
}

/// Statuses that mean "the model saw *these bytes* and refused them" — the
/// request was understood and the payload judged unusable, so the same document
/// fails the same way on every retry.
///
/// Deliberately narrow. Every other non-2xx is about the service, not the
/// document, and that distinction is the whole point: **404** is `model not
/// found, try pulling it first` (never pulled, or a typo'd
/// `OLLAMA_SUMMARY_MODEL`), **500** is most often a model load failure such as
/// `requires more system memory than is available`, and **502/503/504** is a
/// reverse proxy in front of an Ollama that is down or restarting.
///
/// **400 is excluded on purpose**, though it reads like an input rejection: it
/// is ambiguous between "this payload is bad" and "this *request shape* is
/// bad", and the second one — an Ollama API change, or a field we send that it
/// stops accepting — answers 400 for every blob alike. That is the same
/// systemic write-off as the codes above: three restarts and the whole queued
/// backlog is `PermanentlyFailed`, recoverable only one `resummarize_blob` at a
/// time. 413 and 422 carry a claim about the payload specifically; 400 does not.
///
/// The cost of being wrong in this direction is one wasted retry per restart.
/// The cost of being wrong in the other direction is losing documents.
fn is_input_rejection(code: u16) -> bool {
    matches!(code, 413 | 422)
}

/// Did this failure implicate the document, or the dependency?
///
/// Only an input rejection from a reachable model is document-scoped — that is
/// the context-overflow case, where the same bytes fail identically forever and
/// retrying is pure waste. Connection refused, timeouts, malformed responses,
/// and every other non-2xx are the dependency's problem and must not spend a
/// blob's retry budget.
///
/// Anything unrecognised defaults to `Dependency` — the safe direction, since
/// it costs a retry rather than a document.
pub fn classify_ollama_error(err: &AppError) -> crate::models::BlobFailureKind {
    use crate::models::BlobFailureKind;
    let AppError::Internal(msg) = err else {
        return BlobFailureKind::Dependency;
    };
    let Some(rest) = msg.strip_prefix(OLLAMA_STATUS_PREFIX) else {
        return BlobFailureKind::Dependency;
    };
    match rest.split(':').next().and_then(|c| c.trim().parse::<u16>().ok()) {
        Some(code) if is_input_rejection(code) => BlobFailureKind::Document,
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
        let resp = self.client
            .post(format!("{}/api/embeddings", self.base_url))
            .json(&EmbedRequest { model: &self.embed_model, prompt: text })
            .send().await
            .map_err(|e| AppError::Internal(format!("ollama embed: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(status_err("embed", status, &body));
        }
        let parsed: EmbedResponse = resp.json().await
            .map_err(|e| AppError::Internal(format!("ollama embed parse: {e}")))?;
        Ok(parsed.embedding)
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
            return Err(status_err("generate", status, &body));
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
    /// Inputs are built through `status_err` — the same constructor `embed` and
    /// `generate` use — so this cannot pass against a message shape the client
    /// no longer produces.
    #[test]
    fn test_classify_ollama_error() {
        use crate::models::BlobFailureKind;
        let code = |c: u16| reqwest::StatusCode::from_u16(c).unwrap();

        // The model understood the request and refused these bytes.
        for c in [413u16, 422] {
            for op in ["embed", "generate"] {
                let err = status_err(op, code(c), "context length exceeded");
                assert_eq!(
                    classify_ollama_error(&err),
                    BlobFailureKind::Document,
                    "{c} from {op} is an input rejection: {err}"
                );
            }
        }

        // Every other non-2xx is about the service. 404 = model never pulled,
        // 500 = model load / OOM, 502-504 = a proxy over a down Ollama, and 400
        // is ambiguous enough to belong here (a rejected *request shape* hits
        // every blob alike). These must never write a document off.
        for c in [400u16, 404, 408, 429, 500, 502, 503, 504] {
            for op in ["embed", "generate"] {
                let err = status_err(op, code(c), "model requires more system memory");
                assert_eq!(
                    classify_ollama_error(&err),
                    BlobFailureKind::Dependency,
                    "{c} from {op} is the dependency's problem: {err}"
                );
            }
        }

        // Never got an answer at all — says nothing about the document.
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
        assert_eq!(classify_ollama_error(&AppError::NotFound), BlobFailureKind::Dependency);
        assert_eq!(
            classify_ollama_error(&AppError::Internal("ollama status notanumber: x".into())),
            BlobFailureKind::Dependency,
        );
    }

    /// Upstream bodies are persisted per blob and re-written on every restart,
    /// so a proxy's HTML error page must not land in the DB whole.
    #[test]
    fn test_status_err_bounds_the_upstream_body() {
        let huge = "<html>".to_string() + &"x".repeat(50_000) + "</html>";
        let err = status_err("generate", reqwest::StatusCode::BAD_GATEWAY, &huge);
        let AppError::Internal(msg) = err else { panic!("expected Internal") };
        assert!(msg.len() < MAX_ERROR_BODY + 100, "body must be truncated, got {} bytes", msg.len());
        assert!(msg.starts_with(OLLAMA_STATUS_PREFIX), "the status tag must survive truncation");
        assert_eq!(
            classify_ollama_error(&AppError::Internal(msg)),
            crate::models::BlobFailureKind::Dependency,
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
