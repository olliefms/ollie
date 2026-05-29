// src/api/oauth/register.rs
use crate::{models::OAuthClient, AppState};
use axum::{extract::State, http::StatusCode, Json};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use super::OauthError;

#[derive(Deserialize)]
pub struct RegisterRequest {
    #[serde(default)]
    pub client_name: Option<String>,
    #[serde(default)]
    pub redirect_uris: Vec<String>,
}

#[derive(Serialize)]
pub struct RegisterResponse {
    pub client_id: String,
    pub token_endpoint_auth_method: &'static str,
    pub redirect_uris: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
}

/// Script-capable / dangerous URI schemes that must never be accepted as a
/// redirect target, even though a desktop client may register a custom scheme.
const DENIED_SCHEMES: &[&str] = &[
    "javascript", "data", "vbscript", "file", "blob", "about", "mailto", "ftp",
];

/// A scheme name must be well-formed per RFC 3986: `[a-z][a-z0-9+.-]*`
/// (url::Url already lowercases the scheme).
fn is_valid_scheme_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '-' | '.'))
}

/// A redirect URI is acceptable if it is an https URL, a loopback http URL, or
/// a well-formed custom scheme (for native/desktop clients) that is not in the
/// script-capable denylist. Arbitrary non-loopback `http://` is rejected.
fn redirect_uri_ok(uri: &str) -> bool {
    if let Ok(u) = url::Url::parse(uri) {
        match u.scheme() {
            "https" => true,
            "http" => matches!(u.host_str(), Some("127.0.0.1") | Some("localhost") | Some("[::1]")),
            s => is_valid_scheme_name(s) && !DENIED_SCHEMES.contains(&s),
        }
    } else {
        false
    }
}

pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<RegisterResponse>), OauthError> {
    if req.redirect_uris.is_empty() {
        return Err(OauthError::InvalidClientMetadata("redirect_uris required".into()));
    }
    for uri in &req.redirect_uris {
        if !redirect_uri_ok(uri) {
            return Err(OauthError::InvalidClientMetadata(format!("invalid redirect_uri: {uri}")));
        }
    }
    let client = OAuthClient {
        id: Uuid::new_v4(),
        client_name: req.client_name.clone(),
        redirect_uris: req.redirect_uris.clone(),
        created_at: Utc::now(),
    };
    state.db.insert_oauth_client(&client).await
        .map_err(|e| OauthError::ServerError(e.to_string()))?;
    Ok((StatusCode::CREATED, Json(RegisterResponse {
        client_id: client.id.to_string(),
        token_endpoint_auth_method: "none",
        redirect_uris: client.redirect_uris,
        client_name: client.client_name,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redirect_uri_ok() {
        assert!(redirect_uri_ok("https://example.com/cb"));
        assert!(redirect_uri_ok("http://127.0.0.1:33418/callback"));
        assert!(redirect_uri_ok("http://localhost:8080/cb"));
        assert!(redirect_uri_ok("claude://callback"));
        assert!(redirect_uri_ok("cursor://anysphere.cursor-retrieval/oauth/cb"));
        assert!(!redirect_uri_ok("http://evil.com/cb"));
        assert!(!redirect_uri_ok("not a url"));
        // Script-capable / dangerous schemes must be rejected.
        assert!(!redirect_uri_ok("javascript:alert(1)"));
        assert!(!redirect_uri_ok("data:text/html,<script>alert(1)</script>"));
        assert!(!redirect_uri_ok("vbscript:msgbox(1)"));
        assert!(!redirect_uri_ok("file:///etc/passwd"));
        assert!(!redirect_uri_ok("blob:https://x/uuid"));
    }
}
