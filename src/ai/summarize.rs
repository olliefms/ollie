// src/ai/summarize.rs
use crate::{
    ai::{extract::bytes_to_base64, OllamaClient},
    error::AppError,
};
use bytes::Bytes;

pub async fn summarize_text(client: &OllamaClient, text: &str) -> Result<String, AppError> {
    if text.trim().is_empty() {
        return Ok(String::new());
    }
    let truncated = if text.len() > 4000 { &text[..4000] } else { text };
    let prompt = format!(
        "Provide a concise 1-2 sentence summary of the following content. \
        Respond with only the summary, no preamble:\n\n{truncated}"
    );
    client.generate(&client.summary_model.clone(), &prompt, None).await
}

/// Summarize OCR text recovered from a scanned business document.
///
/// Framing the task as indexing the fleet's own records matters: with the
/// bare summarize_text prompt, llama3.2 refuses OCR'd invoices as "sensitive
/// customer data" in ~2 of 3 runs (measured against production Ollama);
/// with this framing it produced accurate summaries 3/3. (#372)
pub async fn summarize_document_text(client: &OllamaClient, text: &str) -> Result<String, AppError> {
    if text.trim().is_empty() {
        return Ok(String::new());
    }
    let truncated = truncate_at_char_boundary(text, 4000);
    let prompt = format!(
        "You are a document-indexing assistant for a trucking fleet's own records. \
        The following is OCR text from one of the fleet's business documents \
        (invoice, receipt, bill of lading, rate confirmation, or similar). \
        Write a concise 1-2 sentence summary for the document index: document type, \
        vendor, date, vehicle or load, key items, and total. \
        Respond with only the summary, no preamble:\n\n{truncated}"
    );
    client.generate(&client.summary_model.clone(), &prompt, None).await
}

/// Truncate to at most `max` bytes without splitting a UTF-8 character —
/// OCR output is arbitrary text and a naive byte slice can panic mid-char.
fn truncate_at_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

pub async fn describe_image(client: &OllamaClient, data: &Bytes) -> Result<String, AppError> {
    let b64 = bytes_to_base64(data);
    let prompt = "Describe the content of this image in 1-2 concise sentences. \
                  If this is a document or contains text, summarize what it says. \
                  Respond with only the description, no preamble.";
    client.generate(&client.vision_model.clone(), prompt, Some(b64)).await
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_summarize_text_short_circuits_on_empty() {
        use super::*;
        use crate::ai::OllamaClient;
        // Unreachable URL — if the guard fails, the HTTP call will error.
        let client = OllamaClient::new("http://127.0.0.1:1", "nomic-embed-text", "llama3.2", "moondream");
        assert_eq!(summarize_text(&client, "").await.unwrap(), "");
        assert_eq!(summarize_text(&client, "   \n\t  ").await.unwrap(), "");
    }

    #[tokio::test]
    async fn test_summarize_document_text_short_circuits_on_empty() {
        use super::*;
        use crate::ai::OllamaClient;
        // Unreachable URL — if the guard fails, the HTTP call will error.
        let client = OllamaClient::new("http://127.0.0.1:1", "nomic-embed-text", "llama3.2", "moondream");
        assert_eq!(summarize_document_text(&client, "").await.unwrap(), "");
        assert_eq!(summarize_document_text(&client, "   \n\t  ").await.unwrap(), "");
    }

    #[test]
    fn test_truncate_at_char_boundary_never_splits_chars() {
        use super::truncate_at_char_boundary;
        assert_eq!(truncate_at_char_boundary("short", 4000), "short");
        // 'é' is 2 bytes; cutting at byte 3 must back off to a boundary.
        let s = "aéé";
        assert_eq!(truncate_at_char_boundary(s, 2), "a");
        assert_eq!(truncate_at_char_boundary(s, 3), "aé");
        assert_eq!(truncate_at_char_boundary(s, 4), "aé");
        assert_eq!(truncate_at_char_boundary(s, 5), "aéé");
    }

    #[tokio::test]
    #[ignore] // requires live Ollama
    async fn test_summarize_text_returns_non_empty() {
        use super::*;
        use crate::ai::OllamaClient;
        let client = OllamaClient::new("http://localhost:11434", "nomic-embed-text", "llama3.2", "moondream");
        let summary = summarize_text(&client, "Rust is a systems programming language focused on safety.").await.unwrap();
        assert!(!summary.is_empty());
    }
}
