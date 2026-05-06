// src/ai/embed.rs
use crate::{ai::OllamaClient, error::AppError};

pub async fn embed_text(client: &OllamaClient, text: &str) -> Result<Vec<f32>, AppError> {
    if text.trim().is_empty() {
        return Err(AppError::Internal("cannot embed empty text".into()));
    }
    // Truncate to ~8000 chars to stay within model context limits
    let truncated = if text.len() > 8000 { &text[..8000] } else { text };
    client.embed(truncated).await
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    #[ignore] // requires live Ollama: cargo test ai::embed -- --ignored
    async fn test_embed_returns_non_empty_vector() {
        use super::*;
        use crate::ai::OllamaClient;
        let client = OllamaClient::new("http://localhost:11434", "nomic-embed-text", "llama3.2", "llava");
        let vec = embed_text(&client, "the quick brown fox").await.unwrap();
        assert!(!vec.is_empty());
    }
}
