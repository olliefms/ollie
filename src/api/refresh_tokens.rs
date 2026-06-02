// src/api/refresh_tokens.rs
//
// Shared refresh-token machinery for the PWA login flows (this plan) and the
// OAuth token endpoint (Plan 2). Access tokens stay short-lived JWTs; these
// opaque, hashed, rotating tokens carry the long-lived session.
use crate::{db::DbClient, error::AppError, models::RefreshToken};
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const REFRESH_TTL_DAYS: i64 = 14;
/// Cookie name for the PWA refresh token.
pub const REFRESH_COOKIE: &str = "ollie_refresh";

pub fn hash_token(plaintext: &str) -> String {
    hex::encode(Sha256::digest(plaintext.as_bytes()))
}

/// 32 bytes of CSPRNG, base64url (no padding), `ollr_`-prefixed for greppability.
fn generate_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    use base64::Engine;
    format!("ollr_{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// The plaintext secret (returned to the caller once) plus the stored row.
pub struct IssuedToken {
    pub secret: String,
    pub record: RefreshToken,
}

/// Mint a brand-new refresh token starting a fresh family. Persists the row.
pub async fn issue(
    db: &DbClient,
    subject_type: &str,
    subject_id: Uuid,
    client_id: Option<Uuid>,
    token_version: i64,
    now: DateTime<Utc>,
) -> Result<IssuedToken, AppError> {
    let secret = generate_secret();
    let record = RefreshToken {
        id: Uuid::new_v4(),
        token_hash: hash_token(&secret),
        subject_type: subject_type.to_string(),
        subject_id,
        client_id,
        family_id: Uuid::new_v4(),
        token_version,
        issued_at: now,
        expires_at: now + Duration::days(REFRESH_TTL_DAYS),
        consumed_at: None,
        revoked_at: None,
        last_used_at: None,
    };
    db.insert_refresh_token(&record).await?;
    Ok(IssuedToken { secret, record })
}

/// Outcome of presenting a refresh token.
pub enum RotateResult {
    /// New secret + new row; the access token should be re-minted with `token_version`.
    Rotated(Box<IssuedToken>),
    /// Token missing/expired/revoked, or `token_version` mismatch — re-auth required.
    Invalid,
    /// A consumed token was replayed: the family has been revoked. Re-auth required.
    ReusedFamilyRevoked,
}

/// Validate + rotate. Consumes the presented row, appends a new row in the same
/// family with a fresh TTL. Replaying a consumed token revokes the whole family.
/// `current_token_version` is the subject's live `token_version` (the kill switch).
pub async fn rotate(
    db: &DbClient,
    presented_secret: &str,
    current_token_version: i64,
    now: DateTime<Utc>,
) -> Result<RotateResult, AppError> {
    let hash = hash_token(presented_secret);
    let row = match db.get_refresh_token_by_hash(&hash).await? {
        Some(r) => r,
        None => return Ok(RotateResult::Invalid),
    };

    if row.revoked_at.is_some() || row.expires_at <= now {
        return Ok(RotateResult::Invalid);
    }
    if row.token_version != current_token_version {
        return Ok(RotateResult::Invalid);
    }
    // Reuse detection: a consumed token presented again ⇒ theft ⇒ revoke family.
    if row.consumed_at.is_some() {
        db.revoke_refresh_token_family(row.family_id, now).await?;
        return Ok(RotateResult::ReusedFamilyRevoked);
    }

    // Consume the presented row.
    let mut consumed = row.clone();
    consumed.consumed_at = Some(now);
    consumed.last_used_at = Some(now);
    db.upsert_refresh_token(&consumed).await?;

    // Append a new row in the same family with a fresh TTL.
    let secret = generate_secret();
    let next = RefreshToken {
        id: Uuid::new_v4(),
        token_hash: hash_token(&secret),
        subject_type: row.subject_type.clone(),
        subject_id: row.subject_id,
        client_id: row.client_id,
        family_id: row.family_id,
        token_version: current_token_version,
        issued_at: now,
        expires_at: now + Duration::days(REFRESH_TTL_DAYS),
        consumed_at: None,
        revoked_at: None,
        last_used_at: None,
    };
    db.insert_refresh_token(&next).await?;
    Ok(RotateResult::Rotated(Box::new(IssuedToken { secret, record: next })))
}

/// `Set-Cookie` value for the refresh token (HttpOnly, SameSite=Lax, Path=/).
/// `secure` adds the `Secure` attribute (true in prod / https).
pub fn set_cookie_header(secret: &str, secure: bool) -> String {
    let max_age = REFRESH_TTL_DAYS * 24 * 3600;
    let mut c = format!(
        "{REFRESH_COOKIE}={secret}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}"
    );
    if secure {
        c.push_str("; Secure");
    }
    c
}

/// `Set-Cookie` value that clears the refresh cookie (logout).
pub fn clear_cookie_header(secure: bool) -> String {
    let mut c = format!("{REFRESH_COOKIE}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0");
    if secure {
        c.push_str("; Secure");
    }
    c
}

/// Extract the refresh cookie value from a Cookie header, if present.
pub fn read_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|kv| {
            let mut parts = kv.trim().splitn(2, '=');
            Some((parts.next()?, parts.next()?))
        })
        .find(|(k, _)| *k == REFRESH_COOKIE)
        .map(|(_, v)| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn test_db() -> (DbClient, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        (db, dir)
    }

    #[tokio::test]
    async fn test_issue_then_rotate_returns_new_secret() {
        let (db, _d) = test_db().await;
        let subj = Uuid::new_v4();
        let issued = issue(&db, "fleet_user", subj, None, 0, Utc::now()).await.unwrap();
        let r = rotate(&db, &issued.secret, 0, Utc::now()).await.unwrap();
        match r {
            RotateResult::Rotated(next) => {
                assert_ne!(next.secret, issued.secret);
                assert_eq!(next.record.family_id, issued.record.family_id);
            }
            _ => panic!("expected Rotated"),
        }
    }

    #[tokio::test]
    async fn test_reused_token_revokes_family() {
        let (db, _d) = test_db().await;
        let subj = Uuid::new_v4();
        let issued = issue(&db, "fleet_user", subj, None, 0, Utc::now()).await.unwrap();
        let _ = rotate(&db, &issued.secret, 0, Utc::now()).await.unwrap();
        let again = rotate(&db, &issued.secret, 0, Utc::now()).await.unwrap();
        assert!(matches!(again, RotateResult::ReusedFamilyRevoked));
        let fam_rows = db.list_refresh_tokens_by_family(issued.record.family_id).await.unwrap();
        assert!(fam_rows.iter().all(|r| r.revoked_at.is_some()));
    }

    #[tokio::test]
    async fn test_token_version_mismatch_is_invalid() {
        let (db, _d) = test_db().await;
        let subj = Uuid::new_v4();
        let issued = issue(&db, "fleet_user", subj, None, 0, Utc::now()).await.unwrap();
        let r = rotate(&db, &issued.secret, 1, Utc::now()).await.unwrap();
        assert!(matches!(r, RotateResult::Invalid));
    }

    #[tokio::test]
    async fn test_expired_token_is_invalid() {
        let (db, _d) = test_db().await;
        let subj = Uuid::new_v4();
        let past = Utc::now() - chrono::Duration::days(15);
        let issued = issue(&db, "fleet_user", subj, None, 0, past).await.unwrap();
        let r = rotate(&db, &issued.secret, 0, Utc::now()).await.unwrap();
        assert!(matches!(r, RotateResult::Invalid));
    }

    #[test]
    fn test_cookie_headers() {
        let set = set_cookie_header("ollr_abc", true);
        assert!(set.contains("ollie_refresh=ollr_abc"));
        assert!(set.contains("HttpOnly") && set.contains("Secure") && set.contains("Max-Age=1209600"));
        let clear = clear_cookie_header(false);
        assert!(clear.contains("Max-Age=0"));
        assert!(!clear.contains("Secure"));
    }

    #[test]
    fn test_read_cookie() {
        let mut h = axum::http::HeaderMap::new();
        h.insert(axum::http::header::COOKIE, "foo=1; ollie_refresh=ollr_xyz; bar=2".parse().unwrap());
        assert_eq!(read_cookie(&h), Some("ollr_xyz".to_string()));
        let empty = axum::http::HeaderMap::new();
        assert_eq!(read_cookie(&empty), None);
    }
}
