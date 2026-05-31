// src/config.rs
use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub admin_api_key: String,
    pub port: u16,
    pub blob_store_path: String,
    pub extract_store_path: String,
    pub lancedb_path: String,
    pub ollama_base_url: String,
    pub ollama_embed_model: String,
    pub ollama_summary_model: String,
    pub ollama_vision_model: String,
    pub ollama_embed_dim: usize,
    pub pipeline_workers: usize,
    pub ors_api_key: String,
    pub facility_dedup_high_threshold: f64,
    pub facility_dedup_low_threshold: f64,
    pub geocoding_workers: usize,
    pub driver_jwt_secret: String,
    pub driver_rp_id: String,
    pub driver_rp_origin: String,
    pub dispatcher_jwt_secret: String,
    /// Externally-reachable base URL (no trailing slash), e.g. `https://ollie.example.com`.
    /// Used to build absolute presigned blob upload/download URLs handed to MCP agents.
    /// Empty when unset — the presigned MCP tools error clearly in that case rather than
    /// emitting an unusable relative URL.
    pub public_base_url: String,
    /// True when `public_base_url` starts with `https://`; used to set the Secure flag on cookies.
    pub cookie_secure: bool,
    /// Default TTL (seconds) for presigned blob URLs when the caller omits one.
    pub blob_presign_ttl_secs: u64,
    /// Hard cap (seconds) on presigned blob URL TTL, regardless of caller request.
    pub blob_presign_max_ttl_secs: u64,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let admin_api_key = env::var("ADMIN_API_KEY")
            .map_err(|_| "ADMIN_API_KEY is required")?;
        let driver_jwt_secret = env::var("DRIVER_JWT_SECRET")
            .map_err(|_| "DRIVER_JWT_SECRET is required")?;
        if driver_jwt_secret.len() < 32 {
            return Err("DRIVER_JWT_SECRET must be at least 32 bytes".into());
        }
        let dispatcher_jwt_secret = env::var("DISPATCHER_JWT_SECRET")
            .map_err(|_| "DISPATCHER_JWT_SECRET is required")?;
        if dispatcher_jwt_secret.len() < 32 {
            return Err("DISPATCHER_JWT_SECRET must be at least 32 bytes".into());
        }
        let driver_rp_id = env::var("DRIVER_RP_ID")
            .map_err(|_| "DRIVER_RP_ID is required")?;
        let driver_rp_origin = env::var("DRIVER_RP_ORIGIN")
            .map_err(|_| "DRIVER_RP_ORIGIN is required")?;
        // NOTE: TERMINAL_TIMEZONE / OLLIE_FREE_DWELL_MINUTES are no longer read here.
        // Terminals own timezone + free-dwell; those env vars are now only first-boot
        // seed values for the Default terminal (see `open_or_create_terminal`).
        let public_base_url = env::var("OLLIE_PUBLIC_BASE_URL")
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        let cookie_secure = public_base_url.starts_with("https://");
        Ok(Self {
            admin_api_key,
            port: env::var("PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(3000),
            blob_store_path: env::var("BLOB_STORE_PATH")
                .unwrap_or_else(|_| "./data/blobs".into()),
            extract_store_path: env::var("EXTRACT_STORE_PATH")
                .unwrap_or_else(|_| "./data/extracts".into()),
            lancedb_path: env::var("LANCEDB_PATH")
                .unwrap_or_else(|_| "./data/lancedb".into()),
            ollama_base_url: env::var("OLLAMA_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434".into()),
            ollama_embed_model: env::var("OLLAMA_EMBED_MODEL")
                .unwrap_or_else(|_| "nomic-embed-text".into()),
            ollama_summary_model: env::var("OLLAMA_SUMMARY_MODEL")
                .unwrap_or_else(|_| "llama3.2".into()),
            ollama_vision_model: env::var("OLLAMA_VISION_MODEL")
                .unwrap_or_else(|_| "moondream".into()),
            ollama_embed_dim: env::var("OLLAMA_EMBED_DIM")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(768),
            pipeline_workers: env::var("PIPELINE_WORKERS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1),
            ors_api_key: env::var("ORS_API_KEY").unwrap_or_default(),
            facility_dedup_high_threshold: env::var("FACILITY_DEDUP_HIGH_THRESHOLD")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(0.92),
            facility_dedup_low_threshold: env::var("FACILITY_DEDUP_LOW_THRESHOLD")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(0.75),
            geocoding_workers: env::var("GEOCODING_WORKERS")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(1),
            driver_jwt_secret,
            driver_rp_id,
            driver_rp_origin,
            dispatcher_jwt_secret,
            public_base_url,
            cookie_secure,
            blob_presign_ttl_secs: env::var("OLLIE_BLOB_PRESIGN_TTL_SECS")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(300),
            blob_presign_max_ttl_secs: env::var("OLLIE_BLOB_PRESIGN_MAX_TTL_SECS")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(3600),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn set_driver_vars() {
        env::set_var("DRIVER_JWT_SECRET", "a-secret-that-is-at-least-32-bytes-long!");
        env::set_var("DRIVER_RP_ID", "localhost");
        env::set_var("DRIVER_RP_ORIGIN", "http://localhost:3000");
        env::set_var("DISPATCHER_JWT_SECRET", "a-dispatcher-secret-at-least-32-bytes!!");
    }

    fn remove_driver_vars() {
        env::remove_var("DRIVER_JWT_SECRET");
        env::remove_var("DRIVER_RP_ID");
        env::remove_var("DRIVER_RP_ORIGIN");
        env::remove_var("DISPATCHER_JWT_SECRET");
    }

    #[test]
    fn test_config_from_env() {
        let _g = ENV_LOCK.lock().unwrap();
        env::set_var("ADMIN_API_KEY", "test-key");
        set_driver_vars();
        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.admin_api_key, "test-key");
        assert_eq!(cfg.port, 3000);
        assert_eq!(cfg.pipeline_workers, 1);
        assert_eq!(cfg.ollama_embed_model, "nomic-embed-text");
        assert_eq!(cfg.ollama_summary_model, "llama3.2");
        assert_eq!(cfg.ollama_vision_model, "moondream");
        assert_eq!(cfg.ollama_embed_dim, 768);
        env::remove_var("ADMIN_API_KEY");
        remove_driver_vars();
    }

    #[test]
    fn test_config_ors_and_dedup_defaults() {
        let _g = ENV_LOCK.lock().unwrap();
        env::set_var("ADMIN_API_KEY", "test-key");
        set_driver_vars();
        env::remove_var("ORS_API_KEY");
        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.ors_api_key, "");
        assert!((cfg.facility_dedup_high_threshold - 0.92).abs() < f64::EPSILON);
        assert!((cfg.facility_dedup_low_threshold - 0.75).abs() < f64::EPSILON);
        assert_eq!(cfg.geocoding_workers, 1);
        env::remove_var("ADMIN_API_KEY");
        remove_driver_vars();
    }

    #[test]
    fn test_config_missing_api_key() {
        let _g = ENV_LOCK.lock().unwrap();
        env::remove_var("ADMIN_API_KEY");
        set_driver_vars();
        assert!(Config::from_env().is_err());
        remove_driver_vars();
    }

    #[test]
    fn test_config_missing_driver_jwt_secret() {
        let _g = ENV_LOCK.lock().unwrap();
        env::set_var("ADMIN_API_KEY", "test-key");
        env::remove_var("DRIVER_JWT_SECRET");
        env::set_var("DRIVER_RP_ID", "localhost");
        env::set_var("DRIVER_RP_ORIGIN", "http://localhost:3000");
        env::set_var("DISPATCHER_JWT_SECRET", "a-dispatcher-secret-at-least-32-bytes!!");
        let result = Config::from_env();
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("DRIVER_JWT_SECRET"), "expected DRIVER_JWT_SECRET in error, got: {msg}");
        env::remove_var("ADMIN_API_KEY");
        remove_driver_vars();
    }

    #[test]
    fn test_config_driver_jwt_secret_too_short() {
        let _g = ENV_LOCK.lock().unwrap();
        env::set_var("ADMIN_API_KEY", "test-key");
        env::set_var("DRIVER_JWT_SECRET", "tooshort");
        env::set_var("DRIVER_RP_ID", "localhost");
        env::set_var("DRIVER_RP_ORIGIN", "http://localhost:3000");
        env::set_var("DISPATCHER_JWT_SECRET", "a-dispatcher-secret-at-least-32-bytes!!");
        let result = Config::from_env();
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("32"), "expected 32 in error, got: {msg}");
        env::remove_var("ADMIN_API_KEY");
        remove_driver_vars();
    }

    #[test]
    fn test_config_all_driver_vars_set() {
        let _g = ENV_LOCK.lock().unwrap();
        env::set_var("ADMIN_API_KEY", "test-key");
        set_driver_vars();
        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.driver_rp_id, "localhost");
        assert_eq!(cfg.driver_rp_origin, "http://localhost:3000");
        assert!(cfg.driver_jwt_secret.len() >= 32);
        env::remove_var("ADMIN_API_KEY");
        remove_driver_vars();
    }
}
