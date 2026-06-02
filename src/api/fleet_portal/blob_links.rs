// src/api/fleet_portal/blob_links.rs
//
// Short-lived, blob-scoped signed URLs ("presigned" URLs) for moving file bytes
// off the MCP transport and onto plain HTTP.
//
// Why this exists: MCP agents authenticate to /fleet/mcp via the MCP client
// layer and never see the fleet user JWT, so they cannot authenticate their own
// HTTP requests. Handing them a full fleet user JWT would let them bypass MCP and
// hit every fleet_user endpoint. Instead, an MCP tool mints a token that is scoped
// to a single blob + operation and expires in minutes. The token rides in the URL
// query string so a header-less GET client (e.g. an agent's `web_fetch`) can use a
// download URL, and any HTTP client can POST bytes to an upload URL.
//
// Tokens are HS256 JWTs signed with the fleet_user secret but carry a distinct
// audience so they can never be used as — or confused with — a fleet_user session
// token.

use crate::error::AppError;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const ISSUER: &str = "ollie-fleet_user";
const AUDIENCE: &str = "ollie-blob-url";

/// The operation a presigned token authorizes. Encoded as the `op` claim so a
/// download token can never be replayed against the upload route or vice versa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobUrlOp {
    /// Download a specific blob's bytes.
    Get,
    /// Create a new blob by POSTing bytes.
    Post,
}

impl BlobUrlOp {
    fn as_str(self) -> &'static str {
        match self {
            BlobUrlOp::Get => "get",
            BlobUrlOp::Post => "post",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlobUrlClaims {
    /// Blob id for download tokens; an opaque upload-ticket id for upload tokens.
    pub sub: String,
    pub op: String,
    pub iss: String,
    pub aud: String,
    pub exp: usize,
    pub iat: usize,
}

/// Mint a presigned token. For `Get`, pass the target blob id; for `Post`, pass
/// `None` (a random ticket id is embedded). Returns the token and its expiry as a
/// Unix timestamp.
pub fn mint_token(
    secret: &str,
    op: BlobUrlOp,
    blob_id: Option<Uuid>,
    ttl_secs: u64,
) -> Result<(String, i64), AppError> {
    let now = jsonwebtoken::get_current_timestamp();
    let exp = now + ttl_secs;
    let sub = match (op, blob_id) {
        (BlobUrlOp::Get, Some(id)) => id.to_string(),
        (BlobUrlOp::Get, None) => return Err(AppError::Internal("get token requires a blob id".into())),
        (BlobUrlOp::Post, _) => Uuid::new_v4().to_string(),
    };
    let claims = BlobUrlClaims {
        sub,
        op: op.as_str().to_string(),
        iss: ISSUER.into(),
        aud: AUDIENCE.into(),
        exp: exp as usize,
        iat: now as usize,
    };
    let token = encode(&Header::default(), &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .map_err(|e| AppError::Internal(format!("blob url token encode error: {e}")))?;
    Ok((token, exp as i64))
}

/// Verify a presigned token, enforcing audience, issuer, expiry, and that the
/// embedded op matches `expected_op`. Returns the decoded claims so callers can
/// additionally check `sub` against a path id for downloads.
pub fn verify_token(secret: &str, token: &str, expected_op: BlobUrlOp) -> Result<BlobUrlClaims, AppError> {
    let mut validation = Validation::default();
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[AUDIENCE]);
    let claims = decode::<BlobUrlClaims>(token, &DecodingKey::from_secret(secret.as_bytes()), &validation)
        .map(|d| d.claims)
        .map_err(|_| AppError::Unauthorized)?;
    if claims.op != expected_op.as_str() {
        return Err(AppError::Unauthorized);
    }
    Ok(claims)
}

/// Build the absolute upload URL an agent POSTs bytes to.
pub fn upload_url(base: &str, token: &str) -> String {
    format!("{base}/fleet/blobs/presigned?token={token}")
}

/// Build the absolute download URL an agent GETs bytes from.
pub fn download_url(base: &str, blob_id: Uuid, token: &str) -> String {
    format!("{base}/fleet/blobs/presigned/{blob_id}?token={token}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "a-fleet_user-secret-at-least-32-bytes!!";

    #[test]
    fn get_token_roundtrip_binds_blob_id() {
        let id = Uuid::new_v4();
        let (token, _exp) = mint_token(SECRET, BlobUrlOp::Get, Some(id), 300).unwrap();
        let claims = verify_token(SECRET, &token, BlobUrlOp::Get).unwrap();
        assert_eq!(claims.sub, id.to_string());
        assert_eq!(claims.op, "get");
    }

    #[test]
    fn post_token_roundtrip() {
        let (token, _exp) = mint_token(SECRET, BlobUrlOp::Post, None, 300).unwrap();
        let claims = verify_token(SECRET, &token, BlobUrlOp::Post).unwrap();
        assert_eq!(claims.op, "post");
    }

    #[test]
    fn wrong_op_rejected() {
        let id = Uuid::new_v4();
        let (token, _exp) = mint_token(SECRET, BlobUrlOp::Get, Some(id), 300).unwrap();
        // A download token must not validate against the upload route.
        assert!(matches!(verify_token(SECRET, &token, BlobUrlOp::Post), Err(AppError::Unauthorized)));
    }

    #[test]
    fn wrong_secret_rejected() {
        let id = Uuid::new_v4();
        let (token, _exp) = mint_token(SECRET, BlobUrlOp::Get, Some(id), 300).unwrap();
        assert!(matches!(
            verify_token("a-different-secret-also-32-bytes-long!!", &token, BlobUrlOp::Get),
            Err(AppError::Unauthorized)
        ));
    }

    #[test]
    fn expired_token_rejected() {
        // exp in the past
        let now = jsonwebtoken::get_current_timestamp();
        let claims = BlobUrlClaims {
            sub: Uuid::new_v4().to_string(),
            op: "get".into(),
            iss: ISSUER.into(),
            aud: AUDIENCE.into(),
            exp: (now - 100) as usize,
            iat: (now - 200) as usize,
        };
        let token = encode(&Header::default(), &claims, &EncodingKey::from_secret(SECRET.as_bytes())).unwrap();
        assert!(matches!(verify_token(SECRET, &token, BlobUrlOp::Get), Err(AppError::Unauthorized)));
    }

    #[test]
    fn fleet_user_session_token_rejected_as_blob_token() {
        // A token minted with the fleet_user session audience must not pass here.
        let token = crate::api::fleet_portal::jwt::encode_fleet_user_jwt(Uuid::new_v4(), 1, SECRET).unwrap();
        assert!(matches!(verify_token(SECRET, &token, BlobUrlOp::Get), Err(AppError::Unauthorized)));
    }

    #[test]
    fn url_builders() {
        let id = Uuid::new_v4();
        assert_eq!(upload_url("https://h", "T"), "https://h/fleet/blobs/presigned?token=T");
        assert_eq!(download_url("https://h", id, "T"), format!("https://h/fleet/blobs/presigned/{id}?token=T"));
    }
}
