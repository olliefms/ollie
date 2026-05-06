// src/ai/summarize.rs
use crate::{
    ai::{extract::bytes_to_base64, OllamaClient},
    error::AppError,
};
use bytes::Bytes;

pub async fn summarize_text(client: &OllamaClient, text: &str) -> Result<String, AppError> {
    let truncated = if text.len() > 4000 { &text[..4000] } else { text };
    let prompt = format!(
        "Provide a concise 1-2 sentence summary of the following content. \
        Respond with only the summary, no preamble:\n\n{truncated}"
    );
    client.generate(&client.summary_model.clone(), &prompt, None).await
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
    #[ignore] // requires live Ollama
    async fn test_summarize_text_returns_non_empty() {
        use super::*;
        use crate::ai::OllamaClient;
        let client = OllamaClient::new("http://localhost:11434", "nomic-embed-text", "llama3.2", "llava");
        let summary = summarize_text(&client, "Rust is a systems programming language focused on safety.").await.unwrap();
        assert!(!summary.is_empty());
    }
}
