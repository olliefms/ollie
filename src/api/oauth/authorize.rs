// src/api/oauth/authorize.rs
use crate::{models::{AuthorizationCode, DispatcherStatus}, AppState};
use axum::{
    extract::{Query, State},
    http::{header::LOCATION, StatusCode},
    response::{Html, IntoResponse, Response},
    Form,
};
use chrono::{Duration, Utc};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Deserialize, Clone)]
pub struct AuthorizeParams {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    #[serde(default)]
    pub code_challenge_method: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub resource: Option<String>,
}

#[derive(Deserialize)]
pub struct AuthorizeForm {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: Option<String>,
    pub state: Option<String>,
    pub scope: Option<String>,
    pub resource: Option<String>,
    pub email: String,
    pub password: String,
    pub decision: String,
}

fn h(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

async fn validate(state: &AppState, client_id: &str, redirect_uri: &str, challenge: &str, method: &Option<String>)
    -> Result<crate::models::OAuthClient, String>
{
    if challenge.is_empty() { return Err("PKCE code_challenge required".into()); }
    if let Some(m) = method { if m != "S256" { return Err("only S256 supported".into()); } }
    let id: Uuid = client_id.parse().map_err(|_| "unknown client".to_string())?;
    let client = state.db.get_oauth_client(id).await
        .map_err(|e| e.to_string())?
        .ok_or("unknown client")?;
    if !client.redirect_uris.iter().any(|u| u == redirect_uri) {
        return Err("redirect_uri mismatch".into());
    }
    Ok(client)
}

pub async fn authorize_page(
    State(state): State<AppState>,
    Query(p): Query<AuthorizeParams>,
) -> Response {
    if p.response_type != "code" {
        return (StatusCode::BAD_REQUEST, "unsupported response_type").into_response();
    }
    let client = match validate(&state, &p.client_id, &p.redirect_uri, &p.code_challenge, &p.code_challenge_method).await {
        Ok(c) => c,
        Err(e) => return (StatusCode::BAD_REQUEST, Html(format!("<h1>Authorization error</h1><p>{}</p>", h(&e)))).into_response(),
    };
    let client_label = client.client_name.clone().unwrap_or_else(|| client.id.to_string());
    let hidden = |k: &str, v: &str| format!(r#"<input type="hidden" name="{k}" value="{}">"#, h(v));
    let page = format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Authorize Ollie</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>body{{font-family:system-ui;max-width:24rem;margin:3rem auto;padding:0 1rem}}
input[type=email],input[type=password]{{display:block;width:100%;padding:.5rem;margin:.4rem 0;box-sizing:border-box}}
button{{padding:.6rem 1rem;margin-right:.5rem}}</style></head>
<body><h1>Connect to Ollie</h1>
<p><strong>{client}</strong> wants to access your Ollie dispatcher account.</p>
<form method="post" action="/oauth/authorize">
{p_rt}{p_cid}{p_ru}{p_cc}{p_ccm}{p_state}{p_scope}{p_res}
<label>Email<input type="email" name="email" required autofocus></label>
<label>Password<input type="password" name="password" required></label>
<button type="submit" name="decision" value="allow">Allow</button>
<button type="submit" name="decision" value="deny">Deny</button>
</form></body></html>"#,
        client = h(&client_label),
        p_rt = hidden("response_type", &p.response_type),
        p_cid = hidden("client_id", &p.client_id),
        p_ru = hidden("redirect_uri", &p.redirect_uri),
        p_cc = hidden("code_challenge", &p.code_challenge),
        p_ccm = hidden("code_challenge_method", p.code_challenge_method.as_deref().unwrap_or("S256")),
        p_state = hidden("state", p.state.as_deref().unwrap_or("")),
        p_scope = hidden("scope", p.scope.as_deref().unwrap_or("")),
        p_res = hidden("resource", p.resource.as_deref().unwrap_or("")),
    );
    Html(page).into_response()
}

fn redirect_with(redirect_uri: &str, query: &str) -> Response {
    let sep = if redirect_uri.contains('?') { '&' } else { '?' };
    let mut r = StatusCode::FOUND.into_response();
    r.headers_mut().insert(LOCATION, format!("{redirect_uri}{sep}{query}").parse().unwrap());
    r
}

pub async fn authorize_decision(
    State(state): State<AppState>,
    Form(f): Form<AuthorizeForm>,
) -> Response {
    let client = match validate(&state, &f.client_id, &f.redirect_uri, &f.code_challenge, &f.code_challenge_method).await {
        Ok(c) => c,
        Err(e) => return (StatusCode::BAD_REQUEST, Html(format!("<h1>Authorization error</h1><p>{}</p>", h(&e)))).into_response(),
    };
    let state_q = f.state.clone().unwrap_or_default();

    if f.decision != "allow" {
        return redirect_with(&f.redirect_uri, &format!("error=access_denied&state={}", urlencode(&state_q)));
    }

    let email = f.email.trim().to_lowercase();
    let dispatcher = match state.db.get_dispatcher_by_email(&email).await {
        Ok(Some(d)) => d,
        _ => {
            // Equalize timing for unknown-email path: run bcrypt against a dummy hash (#107).
            let pw = f.password.clone();
            let dummy = crate::api::dispatcher_portal::auth::dummy_hash().to_string();
            let _ = tokio::task::spawn_blocking(move || bcrypt::verify(&pw, &dummy)).await;
            return (StatusCode::UNAUTHORIZED, Html("<h1>Invalid credentials</h1>".to_string())).into_response();
        }
    };

    if dispatcher.status == DispatcherStatus::Inactive {
        return (StatusCode::UNAUTHORIZED, Html("<h1>Invalid credentials</h1>".to_string())).into_response();
    }

    let mut creds = match state.db.get_dispatcher_credentials(dispatcher.id).await {
        Ok(Some(c)) => c,
        _ => {
            // Equalize timing for missing-credentials path.
            let pw = f.password.clone();
            let dummy = crate::api::dispatcher_portal::auth::dummy_hash().to_string();
            let _ = tokio::task::spawn_blocking(move || bcrypt::verify(&pw, &dummy)).await;
            return (StatusCode::UNAUTHORIZED, Html("<h1>Invalid credentials</h1>".to_string())).into_response();
        }
    };

    if let Some(locked_until) = creds.locked_until {
        if locked_until > Utc::now() {
            return (StatusCode::UNAUTHORIZED, Html("<h1>Invalid credentials</h1>".to_string())).into_response();
        }
    }

    // Shared verify + lockout policy — increments failed_attempts / locks the
    // account on failure, so OAuth can't be used to bypass the login lockout.
    let ok = match crate::api::dispatcher_portal::auth::verify_dispatcher_password(&state, &mut creds, &f.password).await {
        Ok(v) => v,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Html("<h1>Server error</h1>".to_string())).into_response(),
    };
    if !ok {
        return (StatusCode::UNAUTHORIZED, Html("<h1>Invalid credentials</h1>".to_string())).into_response();
    }

    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    use base64::Engine;
    let code = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    let code_hash = hex::encode(Sha256::digest(code.as_bytes()));
    let resource = if f.resource.as_deref().unwrap_or("").is_empty() {
        super::dispatch_resource(&state)
    } else {
        f.resource.clone().unwrap()
    };
    let record = AuthorizationCode {
        code_hash,
        client_id: client.id,
        redirect_uri: f.redirect_uri.clone(),
        code_challenge: f.code_challenge.clone(),
        subject_type: "dispatcher".into(),
        subject_id: dispatcher.id,
        resource,
        scope: if f.scope.as_deref().unwrap_or("").is_empty() { None } else { f.scope.clone() },
        created_at: Utc::now(),
        expires_at: Utc::now() + Duration::minutes(5),
        consumed_at: None,
    };
    if let Err(e) = state.db.insert_authorization_code(&record).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Html(format!("<h1>Server error</h1><p>{}</p>", h(&e.to_string())))).into_response();
    }
    redirect_with(&f.redirect_uri, &format!("code={}&state={}", urlencode(&code), urlencode(&state_q)))
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_h_escapes() {
        assert_eq!(h("<a>&\"b\""), "&lt;a&gt;&amp;&quot;b&quot;");
    }

    #[test]
    fn test_redirect_with_separator() {
        let r1 = redirect_with("https://x/cb", "code=1");
        assert_eq!(r1.headers().get(LOCATION).unwrap(), "https://x/cb?code=1");
        let r2 = redirect_with("https://x/cb?foo=1", "code=1");
        assert_eq!(r2.headers().get(LOCATION).unwrap(), "https://x/cb?foo=1&code=1");
    }
}
