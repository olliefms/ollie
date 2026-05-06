// src/config.rs
use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub admin_api_key: String,
    pub port: u16,
    pub blob_store_path: String,
    pub lancedb_path: String,
    pub ollama_base_url: String,
    pub ollama_embed_model: String,
    pub ollama_summary_model: String,
    pub ollama_vision_model: String,
    pub ollama_embed_dim: usize,
    pub pipeline_workers: usize,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let admin_api_key = env::var("ADMIN_API_KEY")
            .map_err(|_| "ADMIN_API_KEY is required")?;
        Ok(Self {
            admin_api_key,
            port: env::var("PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(3000),
            blob_store_path: env::var("BLOB_STORE_PATH")
                .unwrap_or_else(|_| "./data/blobs".into()),
            lancedb_path: env::var("LANCEDB_PATH")
                .unwrap_or_else(|_| "./data/lancedb".into()),
            ollama_base_url: env::var("OLLAMA_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434".into()),
            ollama_embed_model: env::var("OLLAMA_EMBED_MODEL")
                .unwrap_or_else(|_| "nomic-embed-text".into()),
            ollama_summary_model: env::var("OLLAMA_SUMMARY_MODEL")
                .unwrap_or_else(|_| "llama3.2".into()),
            ollama_vision_model: env::var("OLLAMA_VISION_MODEL")
                .unwrap_or_else(|_| "llava".into()),
            ollama_embed_dim: env::var("OLLAMA_EMBED_DIM")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(768),
            pipeline_workers: env::var("PIPELINE_WORKERS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_from_env() {
        env::set_var("ADMIN_API_KEY", "test-key");
        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.admin_api_key, "test-key");
        assert_eq!(cfg.port, 3000);
        assert_eq!(cfg.pipeline_workers, 1);
        assert_eq!(cfg.ollama_embed_model, "nomic-embed-text");
        assert_eq!(cfg.ollama_summary_model, "llama3.2");
        assert_eq!(cfg.ollama_vision_model, "llava");
        assert_eq!(cfg.ollama_embed_dim, 768);
        env::remove_var("ADMIN_API_KEY");
    }

    #[test]
    fn test_config_missing_api_key() {
        env::remove_var("ADMIN_API_KEY");
        assert!(Config::from_env().is_err());
    }
}
